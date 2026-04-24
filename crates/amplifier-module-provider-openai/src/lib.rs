//! OpenAI Responses API provider for the Amplifier framework.
//!
//! Implements the [`Provider`] trait backed by OpenAI's `/v1/responses` endpoint,
//! with automatic multi-turn continuation when the model hits `max_output_tokens`.

pub mod responses;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use amplifier_core::errors::ProviderError;
use amplifier_core::messages::{
    ChatRequest, ChatResponse, ContentBlock, Message, MessageContent, Role, ToolCall, ToolSpec,
    Usage,
};
use amplifier_core::models::{ModelInfo, ProviderInfo};
use amplifier_core::traits::Provider;
use serde_json::{json, Value};

use crate::responses::{
    ResponsesInputItem, ResponsesReasoning, ResponsesRequest, ResponsesResponse, ResponsesTool,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default base URL for the OpenAI API.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// Default model ID.
pub const DEFAULT_MODEL: &str = "gpt-4o";

/// Default maximum output tokens.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Default maximum retry attempts.
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// Maximum number of auto-continuation attempts when `status == "incomplete"`.
pub const MAX_CONTINUATIONS: usize = 5;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the OpenAI Responses API provider.
#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    /// API key sent as `Authorization: Bearer {api_key}`.
    pub api_key: String,
    /// Model ID (e.g. `"gpt-4o"`).
    pub model: String,
    /// Base URL override (e.g. for testing).
    pub base_url: String,
    /// Maximum output tokens per request.
    pub max_tokens: u32,
    /// Reasoning effort (`"low"`, `"medium"`, `"high"`) for `o*` models.
    pub reasoning_effort: Option<String>,
    /// Maximum number of retry attempts on transient failures.
    pub max_retries: u32,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: DEFAULT_MODEL.to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            reasoning_effort: None,
            max_retries: DEFAULT_MAX_RETRIES,
        }
    }
}

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// OpenAI Responses API provider.
pub struct OpenAIProvider {
    /// Provider configuration.
    pub config: OpenAIConfig,
    /// Reusable HTTP client.
    pub client: reqwest::Client,
}

impl OpenAIProvider {
    /// Create a new provider with the given configuration.
    pub fn new(config: OpenAIConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Wire-format helpers
    // -----------------------------------------------------------------------

    /// Convert Amplifier [`Message`]s to Responses API input items.
    ///
    /// Role mapping:
    /// * `User | Function`        → `"user"`
    /// * `Assistant`              → `"assistant"`
    /// * `System | Developer`     → skipped (sent as `instructions`)
    /// * `Tool`                   → `FunctionCallOutput`
    ///
    /// Content mapping:
    /// * `Text`   → single `Message` item
    /// * `Blocks` → per-block items; `ToolResult` blocks are pushed last
    fn messages_to_input(&self, messages: &[Message]) -> Vec<ResponsesInputItem> {
        let mut items: Vec<ResponsesInputItem> = Vec::new();

        for msg in messages {
            match msg.role {
                // System / Developer messages become `instructions` — skip here.
                Role::System | Role::Developer => continue,

                // Tool messages (function results) become FunctionCallOutput.
                Role::Tool => {
                    let call_id = msg.tool_call_id.clone().unwrap_or_default();
                    let output = match &msg.content {
                        MessageContent::Text(t) => t.clone(),
                        MessageContent::Blocks(blocks) => {
                            serde_json::to_string(blocks).unwrap_or_default()
                        }
                    };
                    items.push(ResponsesInputItem::FunctionCallOutput { call_id, output });
                }

                // Conversational roles.
                Role::User | Role::Function | Role::Assistant => {
                    let role = match msg.role {
                        Role::User | Role::Function => "user",
                        Role::Assistant => "assistant",
                        _ => unreachable!(),
                    };

                    match &msg.content {
                        MessageContent::Text(text) => {
                            items.push(ResponsesInputItem::Message {
                                role: role.to_string(),
                                content: json!(text),
                            });
                        }
                        MessageContent::Blocks(blocks) => {
                            // Collect deferred ToolResult items.
                            let mut tool_results: Vec<ResponsesInputItem> = Vec::new();

                            for block in blocks {
                                match block {
                                    ContentBlock::Text { text, .. } => {
                                        items.push(ResponsesInputItem::Message {
                                            role: role.to_string(),
                                            content: json!(text),
                                        });
                                    }
                                    ContentBlock::ToolCall {
                                        id, name, input, ..
                                    } => {
                                        let arguments =
                                            serde_json::to_string(input).unwrap_or_default();
                                        items.push(ResponsesInputItem::Message {
                                            role: "assistant".to_string(),
                                            content: json!([{
                                                "type": "function_call",
                                                "call_id": id,
                                                "name": name,
                                                "arguments": arguments,
                                            }]),
                                        });
                                    }
                                    ContentBlock::ToolResult {
                                        tool_call_id,
                                        output,
                                        ..
                                    } => {
                                        tool_results.push(ResponsesInputItem::FunctionCallOutput {
                                            call_id: tool_call_id.clone(),
                                            output: output.to_string(),
                                        });
                                    }
                                    _ => {
                                        // Other block types (Image, Thinking, etc.) skipped.
                                    }
                                }
                            }

                            // Push deferred ToolResult items after all other items.
                            items.extend(tool_results);
                        }
                    }
                }
            }
        }

        items
    }

    /// Convert Amplifier [`ToolSpec`]s to Responses API tool definitions.
    ///
    /// Returns `None` when the tool list is empty.
    fn tools_to_responses(&self, tools: &Option<Vec<ToolSpec>>) -> Option<Vec<ResponsesTool>> {
        match tools {
            Some(specs) if !specs.is_empty() => {
                let result = specs
                    .iter()
                    .map(|spec| {
                        let parameters: Value =
                            serde_json::to_value(&spec.parameters).unwrap_or(json!({}));
                        ResponsesTool {
                            tool_type: "function".to_string(),
                            name: spec.name.clone(),
                            description: spec.description.as_deref().unwrap_or("").to_string(),
                            parameters,
                        }
                    })
                    .collect();
                Some(result)
            }
            _ => None,
        }
    }

    /// POST `body` to `{base_url}/v1/responses` with Bearer auth.
    async fn post_request(
        &self,
        body: ResponsesRequest,
    ) -> Result<ResponsesResponse, ProviderError> {
        let url = format!("{}/v1/responses", self.config.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Unavailable {
                message: format!("Request failed: {e}"),
                provider: Some("openai".to_string()),
                model: None,
                retry_after: None,
                status_code: None,
                delay_multiplier: None,
            })?;

        let status_code = response.status().as_u16();

        if response.status().is_success() {
            response
                .json::<ResponsesResponse>()
                .await
                .map_err(|e| ProviderError::Other {
                    message: format!("Failed to parse response: {e}"),
                    provider: Some("openai".to_string()),
                    model: None,
                    retry_after: None,
                    status_code: None,
                    retryable: false,
                    delay_multiplier: None,
                })
        } else {
            let error_body: Value = response.json().await.unwrap_or(json!({}));
            let message = error_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error")
                .to_string();

            match status_code {
                401 | 403 => Err(ProviderError::Authentication {
                    message,
                    provider: Some("openai".to_string()),
                    model: None,
                    retry_after: None,
                }),
                429 => Err(ProviderError::RateLimit {
                    message,
                    provider: Some("openai".to_string()),
                    model: None,
                    retry_after: None,
                    delay_multiplier: None,
                }),
                _ => {
                    let retryable = matches!(status_code, 500 | 502 | 503 | 504);
                    Err(ProviderError::Other {
                        message,
                        provider: Some("openai".to_string()),
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

    /// Extract accumulated assistant text and tool calls from a single response.
    fn extract_text_and_calls(&self, response: &ResponsesResponse) -> (String, Vec<ContentBlock>) {
        let mut text = String::new();
        let mut tool_calls: Vec<ContentBlock> = Vec::new();

        for item in &response.output {
            match item.item_type.as_str() {
                "message" => {
                    if let Some(content) = &item.content {
                        for c in content {
                            if c.content_type == "output_text" {
                                if let Some(t) = &c.text {
                                    text.push_str(t);
                                }
                            }
                        }
                    }
                }
                "function_call" => {
                    let id = item.call_id.clone().unwrap_or_default();
                    let name = item.name.clone().unwrap_or_default();
                    let input: HashMap<String, Value> = item
                        .arguments
                        .as_deref()
                        .and_then(|a| serde_json::from_str(a).ok())
                        .unwrap_or_default();
                    tool_calls.push(ContentBlock::ToolCall {
                        id,
                        name,
                        input,
                        visibility: None,
                        extensions: HashMap::new(),
                    });
                }
                _ => {}
            }
        }

        (text, tool_calls)
    }

    /// Core completion logic with auto-continuation loop.
    async fn do_complete(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        // Extract system instructions from System/Developer messages.
        let instructions: Option<String> = {
            let parts: Vec<String> = request
                .messages
                .iter()
                .filter(|m| matches!(m.role, Role::System | Role::Developer))
                .flat_map(|m| match &m.content {
                    MessageContent::Text(t) => vec![t.clone()],
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
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        };

        let model = request
            .model
            .as_deref()
            .unwrap_or(&self.config.model)
            .to_string();
        let max_output_tokens = request
            .max_output_tokens
            .map(|t| t as u32)
            .unwrap_or(self.config.max_tokens);
        let tools = self.tools_to_responses(&request.tools);
        let reasoning = self
            .config
            .reasoning_effort
            .as_ref()
            .map(|effort| ResponsesReasoning {
                effort: effort.clone(),
            });

        let initial_input = self.messages_to_input(&request.messages);

        let mut req = ResponsesRequest {
            model,
            input: initial_input,
            max_output_tokens,
            instructions,
            tools,
            reasoning,
            include: None,
            previous_response_id: None,
        };

        let mut accumulated_text = String::new();
        let mut accumulated_calls: Vec<ContentBlock> = Vec::new();
        let mut final_usage: Option<Usage> = None;
        let mut finish_reason = "stop".to_string();

        for attempt in 0..=MAX_CONTINUATIONS {
            let response = self.post_request(req.clone()).await?;

            // Accumulate text and tool calls from this response.
            let (text, calls) = self.extract_text_and_calls(&response);
            accumulated_text.push_str(&text);
            accumulated_calls.extend(calls);

            // Update usage from the latest response.
            if let Some(usage) = &response.usage {
                let reasoning_tokens = usage
                    .output_tokens_details
                    .as_ref()
                    .and_then(|d| d.reasoning_tokens)
                    .map(|t| t as i64);
                final_usage = Some(Usage {
                    input_tokens: usage.input_tokens as i64,
                    output_tokens: usage.output_tokens as i64,
                    total_tokens: usage.total_tokens as i64,
                    reasoning_tokens,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    extensions: HashMap::new(),
                });
            }

            match response.status.as_str() {
                "completed" => {
                    finish_reason = "stop".to_string();
                    break;
                }
                "incomplete" => {
                    let reason = response
                        .incomplete_details
                        .as_ref()
                        .map(|d| d.reason.clone())
                        .unwrap_or_default();

                    if reason == "max_output_tokens" && attempt < MAX_CONTINUATIONS {
                        // Push accumulated assistant text so the continuation has context.
                        if !accumulated_text.is_empty() {
                            req.input.push(ResponsesInputItem::Message {
                                role: "assistant".to_string(),
                                content: json!(accumulated_text),
                            });
                        }
                        req.previous_response_id = Some(response.id.clone());
                        req.include = Some(vec!["reasoning.encrypted_content".to_string()]);

                        log::warn!(
                            "openai: response incomplete (max_output_tokens), attempt {}/{}; continuing with previous_response_id={}",
                            attempt + 1,
                            MAX_CONTINUATIONS,
                            response.id,
                        );
                        continue;
                    } else {
                        finish_reason = format!("incomplete:{}", reason);
                        break;
                    }
                }
                other => {
                    return Err(ProviderError::Other {
                        message: format!("Unexpected response status: {other}"),
                        provider: Some("openai".to_string()),
                        model: None,
                        retry_after: None,
                        status_code: None,
                        retryable: false,
                        delay_multiplier: None,
                    });
                }
            }
        }

        // Build final content: text block first, then tool calls.
        let mut content: Vec<ContentBlock> = Vec::new();
        if !accumulated_text.is_empty() {
            content.push(ContentBlock::Text {
                text: accumulated_text,
                visibility: None,
                extensions: HashMap::new(),
            });
        }
        content.extend(accumulated_calls);

        Ok(ChatResponse {
            content,
            tool_calls: None,
            usage: final_usage,
            degradation: None,
            finish_reason: Some(finish_reason),
            metadata: None,
            extensions: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Provider trait implementation
// ---------------------------------------------------------------------------

impl Provider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn get_info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            credential_env_vars: vec!["OPENAI_API_KEY".to_string()],
            capabilities: vec!["tools".to_string(), "reasoning".to_string()],
            defaults: HashMap::new(),
            config_fields: vec![],
        }
    }

    fn list_models(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>, ProviderError>> + Send + '_>> {
        Box::pin(async move {
            Ok(vec![ModelInfo {
                id: self.config.model.clone(),
                display_name: self.config.model.clone(),
                context_window: 128_000,
                max_output_tokens: self.config.max_tokens as i64,
                capabilities: vec!["tools".to_string(), "reasoning".to_string()],
                defaults: HashMap::new(),
            }])
        })
    }

    fn complete(
        &self,
        request: ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        Box::pin(self.do_complete(request))
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
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_model_is_gpt4o() {
        let cfg = OpenAIConfig::default();
        assert_eq!(cfg.model, "gpt-4o");
    }

    #[test]
    fn config_default_base_url() {
        let cfg = OpenAIConfig::default();
        assert_eq!(cfg.base_url, "https://api.openai.com");
    }

    #[test]
    fn config_default_max_tokens() {
        let cfg = OpenAIConfig::default();
        assert_eq!(cfg.max_tokens, 4096);
    }

    #[test]
    fn config_default_max_retries() {
        let cfg = OpenAIConfig::default();
        assert_eq!(cfg.max_retries, 3);
    }

    #[test]
    fn config_default_reasoning_effort_is_none() {
        let cfg = OpenAIConfig::default();
        assert!(cfg.reasoning_effort.is_none());
    }

    #[test]
    fn messages_to_input_user_text() {
        let provider = OpenAIProvider::new(OpenAIConfig::default());
        let messages = vec![Message {
            role: Role::User,
            content: MessageContent::Text("Hello".to_string()),
            name: None,
            tool_call_id: None,
            metadata: None,
            extensions: HashMap::new(),
        }];
        let items = provider.messages_to_input(&messages);
        assert_eq!(items.len(), 1);
        match &items[0] {
            ResponsesInputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content, "Hello");
            }
            other => panic!("Expected Message, got {other:?}"),
        }
    }

    #[test]
    fn messages_to_input_skips_system() {
        let provider = OpenAIProvider::new(OpenAIConfig::default());
        let messages = vec![
            Message {
                role: Role::System,
                content: MessageContent::Text("You are helpful.".to_string()),
                name: None,
                tool_call_id: None,
                metadata: None,
                extensions: HashMap::new(),
            },
            Message {
                role: Role::User,
                content: MessageContent::Text("Hi".to_string()),
                name: None,
                tool_call_id: None,
                metadata: None,
                extensions: HashMap::new(),
            },
        ];
        let items = provider.messages_to_input(&messages);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn messages_to_input_tool_role_becomes_function_call_output() {
        let provider = OpenAIProvider::new(OpenAIConfig::default());
        let messages = vec![Message {
            role: Role::Tool,
            content: MessageContent::Text("result".to_string()),
            name: None,
            tool_call_id: Some("call_abc".to_string()),
            metadata: None,
            extensions: HashMap::new(),
        }];
        let items = provider.messages_to_input(&messages);
        assert_eq!(items.len(), 1);
        match &items[0] {
            ResponsesInputItem::FunctionCallOutput { call_id, output } => {
                assert_eq!(call_id, "call_abc");
                assert_eq!(output, "result");
            }
            other => panic!("Expected FunctionCallOutput, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_calls_extracts_tool_call_blocks() {
        let mut input = HashMap::new();
        input.insert("q".to_string(), json!("rust"));

        let provider = OpenAIProvider::new(OpenAIConfig::default());
        let response = ChatResponse {
            content: vec![
                ContentBlock::Text {
                    text: "Searching...".to_string(),
                    visibility: None,
                    extensions: HashMap::new(),
                },
                ContentBlock::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    input: input.clone(),
                    visibility: None,
                    extensions: HashMap::new(),
                },
            ],
            tool_calls: None,
            usage: None,
            degradation: None,
            finish_reason: None,
            metadata: None,
            extensions: HashMap::new(),
        };
        let calls = provider.parse_tool_calls(&response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "search");
    }
}
