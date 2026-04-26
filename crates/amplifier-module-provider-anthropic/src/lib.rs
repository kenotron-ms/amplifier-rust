//! Anthropic Claude provider for the Amplifier framework.
//!
//! Implements the [`Provider`] trait backed by Anthropic's Messages API,
//! with exponential-backoff retry logic, message-format conversion, and an
//! SSE parser for streaming responses.

/// SSE streaming parser for Anthropic responses.
pub mod streaming;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use amplifier_core::errors::ProviderError;
use amplifier_core::messages::{
    ChatRequest, ChatResponse, ContentBlock, Message, MessageContent, Role, ToolCall, ToolSpec,
    Usage,
};
use amplifier_core::models::{ModelInfo, ProviderInfo};
use amplifier_core::traits::Provider;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Anthropic Messages API endpoint.
pub const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

/// Anthropic API version header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Default maximum number of output tokens.
pub const DEFAULT_MAX_TOKENS: u32 = 32_768;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for [`AnthropicProvider`].
pub struct AnthropicConfig {
    /// API key sent in the `x-api-key` header.
    pub api_key: String,
    /// Default model ID (overridden by per-request `model`).
    pub model: String,
    /// Maximum output tokens for each request.
    pub max_tokens: u32,
    /// Maximum number of retry attempts on transient failures.
    pub max_retries: u32,
    /// Base URL (defaults to [`ANTHROPIC_API_URL`]).
    pub base_url: String,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            max_retries: 3,
            base_url: ANTHROPIC_API_URL.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// Anthropic Claude provider.
pub struct AnthropicProvider {
    /// Provider configuration.
    pub config: AnthropicConfig,
    /// Reusable HTTP client.
    pub client: reqwest::Client,
}

impl AnthropicProvider {
    /// Create a new provider with the given configuration.
    pub fn new(config: AnthropicConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// POST `body` to the configured API URL and return the parsed JSON response.
    ///
    /// Errors are mapped to the appropriate [`ProviderError`] variant.
    /// Codes 429/529/500/502/503/504 are flagged as retryable.
    async fn call_api(&self, body: Value) -> Result<Value, ProviderError> {
        let response = self
            .client
            .post(&self.config.base_url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Unavailable {
                message: format!("Request failed: {e}"),
                provider: Some("anthropic".to_string()),
                model: None,
                retry_after: None,
                status_code: None,
                delay_multiplier: None,
            })?;

        let status_code = response.status().as_u16();

        if response.status().is_success() {
            response
                .json::<Value>()
                .await
                .map_err(|e| ProviderError::Other {
                    message: format!("Failed to parse response body: {e}"),
                    provider: Some("anthropic".to_string()),
                    model: None,
                    retry_after: None,
                    status_code: None,
                    retryable: false,
                    delay_multiplier: None,
                })
        } else {
            let error_body: Value = response.json().await.unwrap_or(json!({}));
            let error_msg = error_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error")
                .to_string();

            match status_code {
                429 => Err(ProviderError::RateLimit {
                    message: error_msg,
                    provider: Some("anthropic".to_string()),
                    model: None,
                    retry_after: None,
                    delay_multiplier: None,
                }),
                401 | 403 => Err(ProviderError::Authentication {
                    message: error_msg,
                    provider: Some("anthropic".to_string()),
                    model: None,
                    retry_after: None,
                }),
                _ => {
                    let retryable = matches!(status_code, 529 | 500 | 502 | 503 | 504);
                    Err(ProviderError::Other {
                        message: error_msg,
                        provider: Some("anthropic".to_string()),
                        model: None,
                        retry_after: None,
                        status_code: Some(status_code),
                        retryable,
                        delay_multiplier: None,
                    })
                }
            }
        }
    }

    /// Call the API with exponential-backoff retry.
    ///
    /// Delays follow `1_000ms × 2^(attempt − 1)` before each retry.
    /// Only retryable errors (429, 529, 5xx) trigger retries.
    async fn call_api_with_retry(&self, body: Value) -> Result<Value, ProviderError> {
        let max_attempts = self.config.max_retries + 1;
        let mut last_error: Option<ProviderError> = None;

        for attempt in 0..max_attempts {
            if attempt > 0 {
                let delay_ms = 1_000u64 * (1u64 << (attempt - 1));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            match self.call_api(body.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    let retryable = e.retryable();
                    last_error = Some(e);
                    if !retryable {
                        break;
                    }
                    // retryable: continue to next attempt (if any remain)
                }
            }
        }

        Err(last_error.expect("call_api_with_retry: no attempts made"))
    }
}

// ---------------------------------------------------------------------------
// Wire-format conversion helpers (pub(crate) for tests)
// ---------------------------------------------------------------------------

/// Convert an Amplifier [`Message`] to the JSON object expected by Anthropic's API.
///
/// Role mapping:
/// * `User | Tool | Function` → `"user"`
/// * `Assistant`              → `"assistant"`
/// * `System | Developer`     → `"system"`
pub(crate) fn message_to_anthropic(msg: &Message) -> Value {
    let role = match msg.role {
        Role::User | Role::Tool | Role::Function => "user",
        Role::Assistant => "assistant",
        Role::System | Role::Developer => "system",
    };

    let content = match &msg.content {
        MessageContent::Text(text) => json!(text),
        MessageContent::Blocks(blocks) => {
            let converted: Vec<Value> = blocks
                .iter()
                .map(content_block_to_anthropic)
                .filter(|v| !v.is_null())
                .collect();
            json!(converted)
        }
    };

    json!({ "role": role, "content": content })
}

/// Convert a single Amplifier [`ContentBlock`] to its Anthropic wire representation.
///
/// * `ToolCall`  → `{type: "tool_use", id, name, input}`
/// * `ToolResult`→ `{type: "tool_result", tool_use_id, content}`
/// * `Text | Thinking | Image` → passthrough (serialised as-is)
/// * All others  → `null`
pub(crate) fn content_block_to_anthropic(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::ToolCall {
            id, name, input, ..
        } => json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        }),
        ContentBlock::ToolResult {
            tool_call_id,
            output,
            ..
        } => {
            // Anthropic tool_result content must be:
            //   (a) a plain string, OR
            //   (b) an array where every item has a "type" field (content blocks)
            //
            // Raw objects, untyped arrays, and anything else must be JSON-serialised
            // to a string first. This covers:
            //   - SkillEngine returning {skill_name, context, body} objects
            //   - Any other tool returning Value::Object
            //   - Arrays that came from re-parsing context history without type fields
            let content = match output {
                Value::String(_) => output.clone(),
                Value::Array(ref arr)
                    if arr.iter().all(|v| v.get("type").is_some()) =>
                {
                    // Properly typed content-block array — pass through as-is
                    output.clone()
                }
                other => Value::String(
                    serde_json::to_string(other).unwrap_or_default()
                ),
            };
            json!({
                "type": "tool_result",
                "tool_use_id": tool_call_id,
                "content": content,
            })
        }
        ContentBlock::Text { .. } | ContentBlock::Thinking { .. } | ContentBlock::Image { .. } => {
            serde_json::to_value(block).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

/// Parse an array of raw Anthropic response content objects into Amplifier [`ContentBlock`]s.
///
/// Handled types:
/// * `"text"` → [`ContentBlock::Text`]
/// * `"tool_use"` / `"server_tool_use"` → [`ContentBlock::ToolCall`]
/// * `"web_search_tool_result"` → [`ContentBlock::ToolResult`]
/// * `"thinking"` → [`ContentBlock::Thinking`]
///
/// Unknown types are silently skipped.
pub(crate) fn parse_content_blocks(blocks: &[Value]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .filter_map(|block| {
            let block_type = block.get("type")?.as_str()?;

            match block_type {
                "text" => {
                    let text = block.get("text")?.as_str()?.to_string();
                    Some(ContentBlock::Text {
                        text,
                        visibility: None,
                        extensions: HashMap::new(),
                    })
                }
                "tool_use" | "server_tool_use" => {
                    let id = block.get("id")?.as_str()?.to_string();
                    let name = block.get("name")?.as_str()?.to_string();
                    let input: HashMap<String, Value> = block
                        .get("input")
                        .and_then(|v| v.as_object())
                        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default();
                    Some(ContentBlock::ToolCall {
                        id,
                        name,
                        input,
                        visibility: None,
                        extensions: HashMap::new(),
                    })
                }
                // Regular tool results from the orchestrator — support both field-name
                // conventions ("tool_use_id" for Anthropic format, "tool_call_id" for our
                // internal format) and both content field names ("content" / "output").
                "tool_result" => {
                    let tool_call_id = block
                        .get("tool_use_id")
                        .or_else(|| block.get("tool_call_id"))
                        .and_then(|v| v.as_str())
                        .map(String::from)?;
                    let output = block
                        .get("content")
                        .or_else(|| block.get("output"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    Some(ContentBlock::ToolResult {
                        tool_call_id,
                        output,
                        visibility: None,
                        extensions: HashMap::new(),
                    })
                }
                "web_search_tool_result" => {
                    let tool_use_id = block.get("tool_use_id")?.as_str()?.to_string();
                    let content = block.get("content").cloned().unwrap_or(Value::Null);
                    Some(ContentBlock::ToolResult {
                        tool_call_id: tool_use_id,
                        output: content,
                        visibility: None,
                        extensions: HashMap::new(),
                    })
                }
                "thinking" => {
                    let thinking = block.get("thinking")?.as_str()?.to_string();
                    let signature = block
                        .get("signature")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string());
                    Some(ContentBlock::Thinking {
                        thinking,
                        signature,
                        visibility: None,
                        content: None,
                        extensions: HashMap::new(),
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// Convert Amplifier [`ToolSpec`]s to the Anthropic `tools` array format.
///
/// Each tool gets `name`, `description`, and `input_schema` (mapped from `parameters`).
pub(crate) fn build_anthropic_tools(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description.as_deref().unwrap_or(""),
                "input_schema": tool.parameters,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Provider trait implementation
// ---------------------------------------------------------------------------

impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn get_info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "anthropic".to_string(),
            display_name: "Anthropic".to_string(),
            credential_env_vars: vec!["ANTHROPIC_API_KEY".to_string()],
            capabilities: vec![
                "streaming".to_string(),
                "tools".to_string(),
                "vision".to_string(),
            ],
            defaults: HashMap::new(),
            config_fields: vec![],
        }
    }

    fn list_models(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>, ProviderError>> + Send + '_>> {
        Box::pin(async move {
            Ok(vec![
                ModelInfo {
                    id: "claude-sonnet-4-5".to_string(),
                    display_name: "Claude Sonnet 4.5".to_string(),
                    context_window: 200_000,
                    max_output_tokens: 32_768,
                    capabilities: vec![
                        "tools".to_string(),
                        "vision".to_string(),
                        "streaming".to_string(),
                    ],
                    defaults: HashMap::new(),
                },
                ModelInfo {
                    id: "claude-haiku-4-5".to_string(),
                    display_name: "Claude Haiku 4.5".to_string(),
                    context_window: 200_000,
                    max_output_tokens: 8_192,
                    capabilities: vec![
                        "tools".to_string(),
                        "vision".to_string(),
                        "streaming".to_string(),
                    ],
                    defaults: HashMap::new(),
                },
            ])
        })
    }

    fn complete(
        &self,
        request: ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        Box::pin(async move {
            // Extract text from System/Developer messages into the top-level `system` field.
            let system_parts: Vec<String> = request
                .messages
                .iter()
                .filter(|m| matches!(m.role, Role::System | Role::Developer))
                .flat_map(|m| match &m.content {
                    MessageContent::Text(text) => vec![text.clone()],
                    MessageContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text, .. } = b {
                                Some(text.clone())
                            } else {
                                None
                            }
                        })
                        .collect(),
                })
                .collect();

            let system_prompt = if system_parts.is_empty() {
                None
            } else {
                Some(system_parts.join("\n"))
            };

            // Filter out System/Developer messages from the messages array.
            let anthropic_messages: Vec<Value> = request
                .messages
                .iter()
                .filter(|m| !matches!(m.role, Role::System | Role::Developer))
                .map(message_to_anthropic)
                .collect();

            // Resolve model and max_tokens (request overrides config).
            let model = request
                .model
                .as_deref()
                .unwrap_or(&self.config.model)
                .to_string();

            let max_tokens = request
                .max_output_tokens
                .map(|t| t as u32)
                .unwrap_or(self.config.max_tokens);

            // Build the request body.
            let mut body = json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": anthropic_messages,
            });

            if let Some(system) = system_prompt {
                body["system"] = json!(system);
            }

            if let Some(tools) = &request.tools {
                if !tools.is_empty() {
                    body["tools"] = json!(build_anthropic_tools(tools));
                }
            }

            if let Some(temp) = request.temperature {
                body["temperature"] = json!(temp);
            }

            // POST with retry.
            let response = self.call_api_with_retry(body).await?;

            // Parse content blocks.
            let content_values: &[Value] = response
                .get("content")
                .and_then(|c| c.as_array())
                .map(|a| a.as_slice())
                .unwrap_or(&[]);

            let content = parse_content_blocks(content_values);

            // Parse usage (cache tokens are mapped from Anthropic's field names).
            let usage = response.get("usage").map(|u| {
                let input_tokens = u.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let output_tokens = u.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let cache_read_tokens = u.get("cache_read_input_tokens").and_then(|v| v.as_i64());
                let cache_write_tokens = u
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_i64());

                Usage {
                    input_tokens,
                    output_tokens,
                    total_tokens: input_tokens + output_tokens,
                    reasoning_tokens: None,
                    cache_read_tokens,
                    cache_write_tokens,
                    extensions: HashMap::new(),
                }
            });

            // `stop_reason` → `finish_reason`.
            let finish_reason = response
                .get("stop_reason")
                .and_then(|r| r.as_str())
                .map(|r| r.to_string());

            Ok(ChatResponse {
                content,
                tool_calls: None,
                usage,
                degradation: None,
                finish_reason,
                metadata: None,
                extensions: HashMap::new(),
            })
        })
    }

    fn parse_tool_calls(&self, response: &ChatResponse) -> Vec<ToolCall> {
        response
            .content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolCall {
                    id, name, input, ..
                } = block
                {
                    Some(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: input.clone(),
                        extensions: HashMap::new(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Unit tests – message conversion
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn empty_ext() -> HashMap<String, Value> {
        HashMap::new()
    }

    // 1. Config default values
    #[test]
    fn config_default() {
        let cfg = AnthropicConfig::default();
        assert_eq!(cfg.model, "claude-sonnet-4-5");
        assert_eq!(cfg.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.base_url, ANTHROPIC_API_URL);
    }

    // 2. User text message → role "user", content string
    #[test]
    fn user_text_message_to_anthropic() {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Text("hello".to_string()),
            name: None,
            tool_call_id: None,
            metadata: None,
            extensions: empty_ext(),
        };
        let v = message_to_anthropic(&msg);
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"], "hello");
    }

    // 3. ToolCall content block → Anthropic "tool_use" type
    #[test]
    fn tool_call_uses_tool_use_type() {
        let mut input = HashMap::new();
        input.insert("path".to_string(), json!("/tmp/test"));

        let block = ContentBlock::ToolCall {
            id: "toolu_01".to_string(),
            name: "read_file".to_string(),
            input,
            visibility: None,
            extensions: empty_ext(),
        };
        let v = content_block_to_anthropic(&block);
        assert_eq!(v["type"], "tool_use");
        assert_eq!(v["id"], "toolu_01");
        assert_eq!(v["name"], "read_file");
        assert_eq!(v["input"]["path"], "/tmp/test");
    }

    // 4. ToolResult content block → Anthropic "tool_result" with tool_use_id
    #[test]
    fn tool_result_uses_tool_use_id() {
        let block = ContentBlock::ToolResult {
            tool_call_id: "toolu_01".to_string(),
            output: json!("file contents"),
            visibility: None,
            extensions: empty_ext(),
        };
        let v = content_block_to_anthropic(&block);
        assert_eq!(v["type"], "tool_result");
        assert_eq!(v["tool_use_id"], "toolu_01");
        assert_eq!(v["content"], "file contents");
        // must NOT use tool_call_id key
        assert!(v.get("tool_call_id").is_none());
    }

    // 5. parse_content_blocks handles tool_use blocks
    #[test]
    fn parse_content_blocks_handles_tool_use() {
        let blocks = vec![json!({
            "type": "tool_use",
            "id": "toolu_abc",
            "name": "bash",
            "input": {"command": "ls"},
        })];
        let parsed = parse_content_blocks(&blocks);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            ContentBlock::ToolCall {
                id, name, input, ..
            } => {
                assert_eq!(id, "toolu_abc");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "ls");
            }
            other => panic!("Expected ToolCall, got {other:?}"),
        }
    }

    // 6. build_anthropic_tools formats tool specs correctly
    #[test]
    fn build_anthropic_tools_format() {
        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!({"x": {"type": "integer"}}));

        let tools = vec![ToolSpec {
            name: "add".to_string(),
            description: Some("Adds two numbers".to_string()),
            parameters,
            extensions: empty_ext(),
        }];

        let result = build_anthropic_tools(&tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "add");
        assert_eq!(result[0]["description"], "Adds two numbers");
        assert_eq!(result[0]["input_schema"]["type"], "object");
        assert!(result[0].get("parameters").is_none());
    }

    // 7. parse_tool_calls extracts ToolCall content blocks into ToolCall structs
    #[test]
    fn parse_tool_calls_extracts_from_content() {
        let mut input = HashMap::new();
        input.insert("query".to_string(), json!("rust serde"));

        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let response = ChatResponse {
            content: vec![
                ContentBlock::Text {
                    text: "Let me search.".to_string(),
                    visibility: None,
                    extensions: empty_ext(),
                },
                ContentBlock::ToolCall {
                    id: "toolu_search".to_string(),
                    name: "search".to_string(),
                    input,
                    visibility: None,
                    extensions: empty_ext(),
                },
            ],
            tool_calls: None,
            usage: None,
            degradation: None,
            finish_reason: None,
            metadata: None,
            extensions: empty_ext(),
        };

        let calls = provider.parse_tool_calls(&response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_search");
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].arguments["query"], "rust serde");
    }
}
