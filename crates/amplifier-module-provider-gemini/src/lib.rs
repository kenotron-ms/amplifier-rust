//! Gemini Developer API provider for the Amplifier framework.

/// Gemini API wire types (requests, responses, content parts).
pub mod types;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use amplifier_core::errors::ProviderError;
use amplifier_core::messages::{
    ChatRequest, ChatResponse, ContentBlock, Message, MessageContent, Role, ToolSpec, Usage,
};
use amplifier_core::models::{ModelInfo, ProviderInfo};
use amplifier_core::traits::Provider;
use futures::StreamExt;
use serde_json::Value;
use types::{
    GeminiContent, GeminiFunctionCall, GeminiFunctionDeclaration, GeminiFunctionResponse,
    GeminiGenerationConfig, GeminiPart, GeminiRequest, GeminiStreamChunk, GeminiSystemInstruction,
    GeminiThinkingConfig, GeminiToolWrapper, GeminiUsageMetadata,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default base URL for the Gemini API (without path).
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Default model.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// Default max output tokens.
pub const DEFAULT_MAX_TOKENS: u32 = 8192;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the Gemini provider.
#[derive(Debug, Clone)]
pub struct GeminiConfig {
    /// API key sent as the `key` query parameter.
    pub api_key: String,
    /// Model ID (e.g. "gemini-2.5-flash").
    pub model: String,
    /// Maximum output tokens per request.
    pub max_tokens: u32,
    /// Thinking budget: -1 = dynamic, 0 = off, N = fixed budget.
    pub thinking_budget: i32,
    /// Maximum number of retry attempts on transient failures.
    pub max_retries: u32,
    /// Base URL override (e.g. for testing with wiremock).
    pub base_url: String,
}

impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            thinking_budget: -1,
            max_retries: 3,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// SSE helpers (kept public for unit tests)
// ---------------------------------------------------------------------------

/// A synthetic function-call extracted from a Gemini SSE stream chunk.
///
/// Gemini does not provide call IDs in its wire format; `id` is generated
/// locally using a UUID v4 prefixed with `gemini_call_`.
#[derive(Debug, Clone)]
pub struct SseFunctionCall {
    /// Synthetic unique ID for this call (format: `gemini_call_<uuid>`).
    pub id: String,
    /// Name of the function the model wants to invoke.
    pub name: String,
    /// Arguments for the function call (arbitrary JSON object).
    pub args: Value,
}

/// Parse a single SSE data line from the Gemini `streamGenerateContent` endpoint.
///
/// Returns `None` when:
/// * `data` is empty
/// * `data` starts with `'['` (e.g. `"[DONE]"`)
/// * the JSON does not contain at least one candidate with content parts
///
/// Otherwise returns `(text, calls)` where `text` is the concatenation of all
/// [`GeminiPart::Text`] parts and `calls` is a list of [`SseFunctionCall`]
/// values derived from all [`GeminiPart::FunctionCall`] parts.
/// [`GeminiPart::FunctionResponse`] parts are ignored.
pub fn parse_sse_line(data: &str) -> Option<(String, Vec<SseFunctionCall>)> {
    if data.is_empty() || data.starts_with('[') {
        return None;
    }

    let chunk: GeminiStreamChunk = serde_json::from_str(data).ok()?;

    let candidates = chunk.candidates?;
    let first = candidates.into_iter().next()?;
    let content = first.content?;

    if content.parts.is_empty() {
        return None;
    }

    let mut text = String::new();
    let mut calls: Vec<SseFunctionCall> = Vec::new();

    for part in content.parts {
        match part {
            GeminiPart::Text { text: t } => {
                text.push_str(&t);
            }
            GeminiPart::FunctionCall { function_call } => {
                calls.push(SseFunctionCall {
                    id: format!("gemini_call_{}", Uuid::new_v4()),
                    name: function_call.name,
                    args: function_call.args,
                });
            }
            GeminiPart::FunctionResponse { .. } => {
                // Ignored per spec.
            }
        }
    }

    Some((text, calls))
}

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// Gemini provider backed by the streaming `streamGenerateContent` endpoint.
pub struct GeminiProvider {
    /// Provider configuration.
    pub config: GeminiConfig,
    /// Reusable HTTP client.
    pub client: reqwest::Client,
}

impl GeminiProvider {
    /// Create a new [`GeminiProvider`] with the given configuration.
    pub fn new(config: GeminiConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Full streaming endpoint URL for the configured model.
    fn url(&self) -> String {
        format!(
            "{}/v1beta/models/{}:streamGenerateContent",
            self.config.base_url, self.config.model
        )
    }

    /// Convert Amplifier messages to Gemini wire format.
    ///
    /// * `User | Tool | Function` → role `"user"`
    /// * `Assistant` → role `"model"`
    /// * `System | Developer` → extracted as system instruction (skipped from contents)
    fn messages_to_gemini(
        &self,
        messages: &[Message],
    ) -> (Vec<GeminiContent>, Option<GeminiSystemInstruction>) {
        let mut system_parts: Vec<GeminiPart> = Vec::new();
        let mut contents: Vec<GeminiContent> = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System | Role::Developer => {
                    let text = match &msg.content {
                        MessageContent::Text(t) => t.clone(),
                        MessageContent::Blocks(blocks) => blocks
                            .iter()
                            .filter_map(|b| {
                                if let ContentBlock::Text { text, .. } = b {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(""),
                    };
                    if !text.is_empty() {
                        system_parts.push(GeminiPart::Text { text });
                    }
                }
                Role::User | Role::Tool | Role::Function => {
                    let parts = self.content_to_parts(&msg.content);
                    if !parts.is_empty() {
                        contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts,
                        });
                    }
                }
                Role::Assistant => {
                    let parts = self.content_to_parts(&msg.content);
                    if !parts.is_empty() {
                        contents.push(GeminiContent {
                            role: "model".to_string(),
                            parts,
                        });
                    }
                }
            }
        }

        let system_instruction = if system_parts.is_empty() {
            None
        } else {
            Some(GeminiSystemInstruction {
                parts: system_parts,
            })
        };

        (contents, system_instruction)
    }

    /// Convert a [`MessageContent`] to a list of [`GeminiPart`]s.
    fn content_to_parts(&self, content: &MessageContent) -> Vec<GeminiPart> {
        match content {
            MessageContent::Text(text) => vec![GeminiPart::Text { text: text.clone() }],
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => {
                        Some(GeminiPart::Text { text: text.clone() })
                    }
                    ContentBlock::ToolCall { name, input, .. } => {
                        let args = serde_json::to_value(input).unwrap_or(Value::Null);
                        Some(GeminiPart::FunctionCall {
                            function_call: GeminiFunctionCall {
                                name: name.clone(),
                                args,
                            },
                        })
                    }
                    ContentBlock::ToolResult { output, .. } => Some(GeminiPart::FunctionResponse {
                        function_response: GeminiFunctionResponse {
                            name: "tool".to_string(),
                            response: output.clone(),
                        },
                    }),
                    _ => None,
                })
                .collect(),
        }
    }

    /// Convert Amplifier [`ToolSpec`]s to the Gemini `tools` array format.
    ///
    /// Returns `None` when the slice is empty.
    fn tools_to_gemini(&self, tools: &[ToolSpec]) -> Option<Vec<GeminiToolWrapper>> {
        if tools.is_empty() {
            return None;
        }
        let declarations: Vec<GeminiFunctionDeclaration> = tools
            .iter()
            .map(|tool| GeminiFunctionDeclaration {
                name: tool.name.clone(),
                description: tool.description.as_deref().unwrap_or("").to_string(),
                parameters: serde_json::to_value(&tool.parameters).unwrap_or(Value::Null),
            })
            .collect();

        Some(vec![GeminiToolWrapper {
            function_declarations: declarations,
        }])
    }

    /// Core async implementation for [`Provider::complete`].
    async fn do_complete(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let (contents, system_instruction) = self.messages_to_gemini(&request.messages);

        let tools = request
            .tools
            .as_deref()
            .and_then(|t| self.tools_to_gemini(t));

        let max_output_tokens = request
            .max_output_tokens
            .map(|t| t as u32)
            .unwrap_or(self.config.max_tokens);

        let thinking_config = if self.config.thinking_budget == 0 {
            None
        } else {
            Some(GeminiThinkingConfig {
                thinking_budget: self.config.thinking_budget,
            })
        };

        let gemini_request = GeminiRequest {
            contents,
            system_instruction,
            tools,
            generation_config: GeminiGenerationConfig {
                max_output_tokens,
                thinking_config,
            },
        };

        // POST to the streaming endpoint with auth key and SSE alt format.
        let response = self
            .client
            .post(self.url())
            .query(&[("key", self.config.api_key.as_str()), ("alt", "sse")])
            .json(&gemini_request)
            .send()
            .await
            .map_err(|e| ProviderError::Unavailable {
                message: format!("Request failed: {e}"),
                provider: Some("gemini".to_string()),
                model: None,
                retry_after: None,
                status_code: None,
                delay_multiplier: None,
            })?;

        let status_code = response.status().as_u16();

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Other {
                message: body,
                provider: Some("gemini".to_string()),
                model: None,
                retry_after: None,
                status_code: Some(status_code),
                retryable: matches!(status_code, 429 | 500 | 502 | 503 | 504),
                delay_multiplier: None,
            });
        }

        // Stream and buffer the SSE byte stream.
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut full_text = String::new();
        let mut tool_call_blocks: Vec<ContentBlock> = Vec::new();
        let mut usage_meta: Option<GeminiUsageMetadata> = None;
        let mut finish_reason: Option<String> = None;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| ProviderError::Unavailable {
                message: format!("Stream error: {e}"),
                provider: Some("gemini".to_string()),
                model: None,
                retry_after: None,
                status_code: None,
                delay_multiplier: None,
            })?;

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process all complete SSE events delimited by \n\n.
            while let Some(pos) = buffer.find("\n\n") {
                let event = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                // Find the `data: ` line within the event.
                let data = event
                    .lines()
                    .find(|line| line.starts_with("data: "))
                    .map(|line| &line["data: ".len()..])
                    .unwrap_or("");

                if data.is_empty() {
                    continue;
                }

                // Accumulate text and function calls via the shared SSE parser.
                if let Some((text, calls)) = parse_sse_line(data) {
                    full_text.push_str(&text);
                    for call in calls {
                        let input: HashMap<String, Value> = call
                            .args
                            .as_object()
                            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                            .unwrap_or_default();
                        tool_call_blocks.push(ContentBlock::ToolCall {
                            id: call.id,
                            name: call.name,
                            input,
                            visibility: None,
                            extensions: HashMap::new(),
                        });
                    }
                }

                // Also extract usage_metadata and finish_reason from the raw chunk.
                if let Ok(chunk_data) = serde_json::from_str::<GeminiStreamChunk>(data) {
                    if let Some(meta) = chunk_data.usage_metadata {
                        usage_meta = Some(meta);
                    }
                    if let Some(candidates) = chunk_data.candidates {
                        for candidate in candidates {
                            if let Some(reason) = candidate.finish_reason {
                                finish_reason = Some(reason);
                            }
                        }
                    }
                }
            }
        }

        // Build the response content: text block (if any) followed by tool calls.
        let mut content: Vec<ContentBlock> = Vec::new();
        if !full_text.is_empty() {
            content.push(ContentBlock::Text {
                text: full_text,
                visibility: None,
                extensions: HashMap::new(),
            });
        }
        content.extend(tool_call_blocks);

        let usage = usage_meta.map(|meta| Usage {
            input_tokens: meta.prompt_token_count as i64,
            output_tokens: meta.candidates_token_count as i64,
            total_tokens: (meta.prompt_token_count + meta.candidates_token_count) as i64,
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
            finish_reason,
            metadata: None,
            extensions: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Provider trait implementation
// ---------------------------------------------------------------------------

impl Provider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn get_info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "gemini".to_string(),
            display_name: "Google Gemini".to_string(),
            credential_env_vars: vec!["GEMINI_API_KEY".to_string()],
            capabilities: vec!["streaming".to_string(), "tools".to_string()],
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
                display_name: "Gemini".to_string(),
                context_window: 1_000_000,
                max_output_tokens: self.config.max_tokens as i64,
                capabilities: vec!["streaming".to_string(), "tools".to_string()],
                defaults: HashMap::new(),
            }])
        })
    }

    fn complete(
        &self,
        request: ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        Box::pin(async move { self.do_complete(request).await })
    }

    fn parse_tool_calls(&self, response: &ChatResponse) -> Vec<amplifier_core::messages::ToolCall> {
        response
            .content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolCall {
                    id, name, input, ..
                } = block
                {
                    Some(amplifier_core::messages::ToolCall {
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
    fn config_default_values() {
        let cfg = GeminiConfig::default();
        assert_eq!(cfg.model, "gemini-2.5-flash");
        assert_eq!(cfg.max_tokens, 8192);
        assert_eq!(cfg.thinking_budget, -1);
        assert_eq!(cfg.max_retries, 3);
        assert!(cfg.api_key.is_empty());
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    }
}
