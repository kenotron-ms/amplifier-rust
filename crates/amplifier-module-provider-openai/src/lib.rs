//! OpenAI Responses API provider for the Amplifier framework.

pub mod responses;

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
}
