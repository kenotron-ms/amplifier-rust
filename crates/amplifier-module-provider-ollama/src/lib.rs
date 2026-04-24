//! Ollama / ChatCompletions-compatible provider for the Amplifier framework.
//!
//! Implements the [`Provider`] trait backed by the `/v1/chat/completions`
//! endpoint (OpenAI-compatible). Compatible with Ollama, LM Studio, vLLM,
//! and OpenRouter.
//!
//! The API key is optional; only attached when `api_key` is `Some`.
//! The `model` field is required — no default model is chosen.

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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default base URL for the Ollama server.
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Default maximum output tokens.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Default maximum retry attempts.
pub const DEFAULT_MAX_RETRIES: u32 = 2;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the Ollama / ChatCompletions-compatible provider.
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// Base URL of the OpenAI-compatible endpoint
    /// (e.g. `"http://localhost:11434"` for Ollama).
    pub base_url: String,
    /// Optional API key sent as `Authorization: Bearer {api_key}`.
    /// Many local servers (Ollama, LM Studio) do not require a key.
    pub api_key: Option<String>,
    /// Model ID — **required**, no default is assumed.
    pub model: String,
    /// Maximum output tokens per request.
    pub max_tokens: u32,
    /// Maximum retry attempts on transient failures.
    pub max_retries: u32,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: None,
            model: String::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
            max_retries: DEFAULT_MAX_RETRIES,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal wire types
// ---------------------------------------------------------------------------

/// Request body for POST /v1/chat/completions.
#[derive(Debug, Serialize)]
struct ChatCompletionsRequest {
    model: String,
    messages: Vec<Value>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    stream: bool,
}

/// Top-level response from POST /v1/chat/completions.
#[derive(Debug, Deserialize)]
struct ChatCompletionsResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

/// One entry in `choices`.
#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// The message in a choice.
#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[allow(dead_code)]
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCall>>,
}

/// A tool call entry inside a message.
#[derive(Debug, Deserialize)]
struct ChatToolCall {
    id: String,
    function: ChatToolCallFunction,
}

/// Function detail inside a tool call.
#[derive(Debug, Deserialize)]
struct ChatToolCallFunction {
    name: String,
    /// Raw JSON-encoded arguments string (e.g. `"{\"url\":\"…\"}"`)
    arguments: String,
}

/// Token usage in the response.
#[derive(Debug, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// Ollama / ChatCompletions-compatible provider.
pub struct OllamaProvider {
    /// Provider configuration.
    pub config: OllamaConfig,
    /// Reusable HTTP client.
    pub client: reqwest::Client,
}

impl OllamaProvider {
    /// Create a new provider with the given configuration.
    pub fn new(config: OllamaConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Wire-format helpers
    // -----------------------------------------------------------------------

    /// Convert a single [`Message`] into one or more ChatCompletions JSON
    /// message objects.
    ///
    /// Role mapping:
    /// * `System | Developer` → `"system"`
    /// * `User | Function`    → `"user"`
    /// * `Assistant`          → `"assistant"`
    /// * `Tool`               → `"tool"`
    ///
    /// Content mapping:
    /// * `Text`   → single message
    /// * `Blocks` → per-block messages
    ///   - `Text` block   → user/assistant message
    ///   - `ToolCall`     → assistant message with `tool_calls` array
    ///   - `ToolResult`   → tool message with `tool_call_id`
    fn message_to_chat(msg: &Message) -> Vec<Value> {
        let role = match msg.role {
            Role::System | Role::Developer => "system",
            Role::User | Role::Function => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };

        match &msg.content {
            MessageContent::Text(text) => {
                vec![json!({ "role": role, "content": text })]
            }
            MessageContent::Blocks(blocks) => {
                let mut out: Vec<Value> = Vec::new();
                for block in blocks {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            out.push(json!({ "role": role, "content": text }));
                        }
                        ContentBlock::ToolCall {
                            id, name, input, ..
                        } => {
                            let arguments = serde_json::to_string(input).unwrap_or_default();
                            out.push(json!({
                                "role": "assistant",
                                "content": null,
                                "tool_calls": [{
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": arguments,
                                    }
                                }]
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_call_id,
                            output,
                            ..
                        } => {
                            out.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_call_id,
                                "content": output.to_string(),
                            }));
                        }
                        _ => {
                            // Other block types (Image, Thinking, etc.) skipped.
                        }
                    }
                }
                out
            }
        }
    }

    /// Convert optional [`ToolSpec`]s to the `tools` field of
    /// ChatCompletions requests.
    ///
    /// Returns `None` when the list is empty or absent.
    fn tools_to_chat(tools: &Option<Vec<ToolSpec>>) -> Option<Vec<Value>> {
        match tools {
            Some(specs) if !specs.is_empty() => {
                let result = specs
                    .iter()
                    .map(|spec| {
                        let parameters: Value =
                            serde_json::to_value(&spec.parameters).unwrap_or(json!({}));
                        json!({
                            "type": "function",
                            "function": {
                                "name": spec.name,
                                "description": spec.description.as_deref().unwrap_or(""),
                                "parameters": parameters,
                            }
                        })
                    })
                    .collect();
                Some(result)
            }
            _ => None,
        }
    }

    /// Core completion logic.
    async fn do_complete(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let model = request
            .model
            .as_deref()
            .unwrap_or(&self.config.model)
            .to_string();

        let max_tokens = request
            .max_output_tokens
            .map(|t| t as u32)
            .unwrap_or(self.config.max_tokens);

        // Build messages list:
        // 1. Prepend system message if present.
        // 2. Convert and append remaining messages.
        let mut messages: Vec<Value> = Vec::new();

        let system_texts: Vec<String> = request
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

        if !system_texts.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": system_texts.join("\n"),
            }));
        }

        for msg in &request.messages {
            if matches!(msg.role, Role::System | Role::Developer) {
                // Already captured as system message above.
                continue;
            }
            messages.extend(Self::message_to_chat(msg));
        }

        let tools = Self::tools_to_chat(&request.tools);

        let body = ChatCompletionsRequest {
            model,
            messages,
            max_tokens,
            tools,
            stream: false,
        };

        let url = format!("{}/v1/chat/completions", self.config.base_url);

        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body);

        // Attach Authorization header only when api_key is Some.
        if let Some(api_key) = &self.config.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let http_response = req_builder
            .send()
            .await
            .map_err(|e| ProviderError::Unavailable {
                message: format!("Request failed: {e}"),
                provider: Some("ollama".to_string()),
                model: None,
                retry_after: None,
                status_code: None,
                delay_multiplier: None,
            })?;

        let status_code = http_response.status().as_u16();

        if !http_response.status().is_success() {
            let error_body: Value = http_response.json().await.unwrap_or(json!({}));
            let message = error_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown API error")
                .to_string();

            return Err(ProviderError::Other {
                message,
                provider: Some("ollama".to_string()),
                model: None,
                retry_after: None,
                status_code: Some(status_code),
                retryable: matches!(status_code, 500 | 502 | 503 | 504),
                delay_multiplier: None,
            });
        }

        let api_response: ChatCompletionsResponse =
            http_response
                .json()
                .await
                .map_err(|e| ProviderError::Other {
                    message: format!("Failed to parse response: {e}"),
                    provider: Some("ollama".to_string()),
                    model: None,
                    retry_after: None,
                    status_code: None,
                    retryable: false,
                    delay_multiplier: None,
                })?;

        // Take the first choice.
        let choice =
            api_response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| ProviderError::Other {
                    message: "Empty choices in response".to_string(),
                    provider: Some("ollama".to_string()),
                    model: None,
                    retry_after: None,
                    status_code: None,
                    retryable: false,
                    delay_multiplier: None,
                })?;

        // Build content blocks.
        let mut content: Vec<ContentBlock> = Vec::new();

        // Text content.
        if let Some(text) = choice.message.content.filter(|t| !t.is_empty()) {
            content.push(ContentBlock::Text {
                text,
                visibility: None,
                extensions: HashMap::new(),
            });
        }

        // Tool call blocks.
        if let Some(tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                let input: HashMap<String, Value> =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                content.push(ContentBlock::ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    input,
                    visibility: None,
                    extensions: HashMap::new(),
                });
            }
        }

        // Build Usage.
        let usage = api_response.usage.map(|u| Usage {
            input_tokens: u.prompt_tokens.unwrap_or(0) as i64,
            output_tokens: u.completion_tokens.unwrap_or(0) as i64,
            total_tokens: u.total_tokens.unwrap_or(0) as i64,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            extensions: HashMap::new(),
        });

        Ok(ChatResponse {
            content,
            tool_calls: None,
            usage,
            degradation: None,
            finish_reason: choice.finish_reason,
            metadata: None,
            extensions: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Provider trait implementation
// ---------------------------------------------------------------------------

impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn get_info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "ollama".to_string(),
            display_name: "Ollama".to_string(),
            credential_env_vars: vec![],
            capabilities: vec!["tools".to_string()],
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
                capabilities: vec!["tools".to_string()],
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
    fn config_default_base_url() {
        let cfg = OllamaConfig::default();
        assert_eq!(cfg.base_url, "http://localhost:11434");
    }

    #[test]
    fn config_default_api_key_is_none() {
        let cfg = OllamaConfig::default();
        assert!(cfg.api_key.is_none());
    }

    #[test]
    fn config_default_model_is_empty() {
        let cfg = OllamaConfig::default();
        assert_eq!(cfg.model, "");
    }

    #[test]
    fn config_default_max_tokens() {
        let cfg = OllamaConfig::default();
        assert_eq!(cfg.max_tokens, 4096);
    }

    #[test]
    fn config_default_max_retries() {
        let cfg = OllamaConfig::default();
        assert_eq!(cfg.max_retries, 2);
    }

    #[test]
    fn provider_name_is_ollama() {
        let provider = OllamaProvider::new(OllamaConfig::default());
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn message_to_chat_user_text() {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Text("Hello".to_string()),
            name: None,
            tool_call_id: None,
            metadata: None,
            extensions: HashMap::new(),
        };
        let items = OllamaProvider::message_to_chat(&msg);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[0]["content"], "Hello");
    }

    #[test]
    fn message_to_chat_system_text() {
        let msg = Message {
            role: Role::System,
            content: MessageContent::Text("Be helpful.".to_string()),
            name: None,
            tool_call_id: None,
            metadata: None,
            extensions: HashMap::new(),
        };
        let items = OllamaProvider::message_to_chat(&msg);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "system");
    }

    #[test]
    fn tools_to_chat_empty_returns_none() {
        assert!(OllamaProvider::tools_to_chat(&None).is_none());
        assert!(OllamaProvider::tools_to_chat(&Some(vec![])).is_none());
    }

    #[test]
    fn tools_to_chat_converts_spec() {
        let specs = Some(vec![ToolSpec {
            name: "search".to_string(),
            description: Some("Search the web".to_string()),
            parameters: HashMap::new(),
            extensions: HashMap::new(),
        }]);
        let tools = OllamaProvider::tools_to_chat(&specs).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "search");
    }
}
