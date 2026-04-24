//! Gemini Developer API provider for the Amplifier framework.

pub mod types;

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
