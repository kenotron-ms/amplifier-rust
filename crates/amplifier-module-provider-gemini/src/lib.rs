//! Gemini Developer API provider for the Amplifier framework.

pub mod types;

use serde_json::Value;
use types::{GeminiPart, GeminiStreamChunk};
use uuid::Uuid;

/// Endpoint: POST https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?key={api_key}
/// Auth is via query param `key=`, not Authorization header.
pub const GEMINI_ENDPOINT_BASE: &str =
    "https://generativelanguage.googleapis.com/v1beta/models";

/// Default model.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// Default max output tokens.
pub const DEFAULT_MAX_TOKENS: u32 = 8192;

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
}

impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            thinking_budget: -1,
            max_retries: 3,
        }
    }
}

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
    }
}
