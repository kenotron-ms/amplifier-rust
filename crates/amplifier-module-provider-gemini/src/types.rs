//! Gemini Developer API wire types.
//!
//! Endpoint: POST https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?key={api_key}
//! Auth: query param `key=` (NOT Authorization header).

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Top-level request body for the Gemini generateContent / streamGenerateContent API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiRequest {
    /// Ordered list of conversation turns.
    pub contents: Vec<GeminiContent>,
    /// Optional system instruction (sent outside the `contents` array).
    #[serde(skip_serializing_if = "Option::is_none", rename = "systemInstruction")]
    pub system_instruction: Option<GeminiSystemInstruction>,
    /// Optional tool declarations made available to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiToolWrapper>>,
    /// Generation parameters (token limits, thinking config, etc.).
    #[serde(rename = "generationConfig")]
    pub generation_config: GeminiGenerationConfig,
}

/// A single turn in the conversation (user or model).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiContent {
    /// Either `"user"` or `"model"`.
    pub role: String,
    /// Parts that make up this content turn.
    pub parts: Vec<GeminiPart>,
}

/// A single part within a [`GeminiContent`] turn.
///
/// Serialised as an untagged union so the presence of the inner field name
/// (`text`, `functionCall`, or `functionResponse`) determines the variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GeminiPart {
    /// Plain text output or input.
    Text {
        /// The text content.
        text: String,
    },
    /// A tool-call request from the model.
    FunctionCall {
        /// The function call details.
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    /// The result of a tool call, provided by the caller.
    FunctionResponse {
        /// The function response details.
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
}

/// A function-call request issued by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionCall {
    /// Name of the function to call.
    pub name: String,
    /// Arguments to pass to the function (JSON object).
    pub args: Value,
}

/// The result of executing a function call, returned to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionResponse {
    /// Name of the function that was called.
    pub name: String,
    /// The function's return value (JSON object).
    pub response: Value,
}

/// System instruction wrapper (Gemini uses `parts` to hold the instruction text).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiSystemInstruction {
    /// Parts that form the system instruction (typically a single `Text` part).
    pub parts: Vec<GeminiPart>,
}

/// Wrapper that groups a list of function declarations into a single tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiToolWrapper {
    /// The function declarations exposed to the model.
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

/// Schema for a single callable function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionDeclaration {
    /// Unique function name.
    pub name: String,
    /// Human-readable description shown to the model.
    pub description: String,
    /// JSON Schema describing the function's parameters.
    pub parameters: Value,
}

/// Per-request generation parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiGenerationConfig {
    /// Maximum number of tokens the model may generate.
    #[serde(rename = "maxOutputTokens")]
    pub max_output_tokens: u32,
    /// Optional thinking / extended-reasoning budget.
    #[serde(skip_serializing_if = "Option::is_none", rename = "thinkingConfig")]
    pub thinking_config: Option<GeminiThinkingConfig>,
}

/// Controls the model's extended-thinking ("thinking") feature.
///
/// `thinking_budget` values:
/// * `-1` — dynamic budget (model decides)
/// * `0`  — thinking disabled
/// * `N`  — fixed budget of *N* tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiThinkingConfig {
    /// Token budget for extended thinking. `-1` = dynamic, `0` = off, `N` = fixed.
    #[serde(rename = "thinkingBudget")]
    pub thinking_budget: i32,
}

// ---------------------------------------------------------------------------
// Response / streaming types
// ---------------------------------------------------------------------------

/// A single chunk received from the streaming `streamGenerateContent` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiStreamChunk {
    /// One or more response candidates (usually 1).
    pub candidates: Option<Vec<GeminiCandidate>>,
    /// Token usage metadata (present on the final chunk).
    #[serde(rename = "usageMetadata")]
    pub usage_metadata: Option<GeminiUsageMetadata>,
}

/// One response candidate within a stream chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiCandidate {
    /// Content parts produced by the model for this candidate.
    pub content: Option<GeminiContent>,
    /// Reason the model stopped generating (`"STOP"`, `"MAX_TOKENS"`, etc.).
    #[serde(rename = "finishReason")]
    pub finish_reason: Option<String>,
}

/// Token usage statistics reported by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiUsageMetadata {
    /// Number of tokens in the prompt.
    #[serde(rename = "promptTokenCount")]
    pub prompt_token_count: u32,
    /// Number of tokens in all candidates combined.
    #[serde(rename = "candidatesTokenCount")]
    pub candidates_token_count: u32,
}

// ---------------------------------------------------------------------------
// Tests — compile-time shape verification and round-trip serialisation
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// GeminiPart::Text serialises to `{"text": "..."}`.
    #[test]
    fn gemini_part_text_roundtrip() {
        let part = GeminiPart::Text {
            text: "hello".to_string(),
        };
        let v = serde_json::to_value(&part).unwrap();
        assert_eq!(v["text"], "hello");
        assert!(v.get("functionCall").is_none());

        let back: GeminiPart = serde_json::from_value(v).unwrap();
        assert!(matches!(back, GeminiPart::Text { text } if text == "hello"));
    }

    /// GeminiPart::FunctionCall serialises with camelCase key.
    #[test]
    fn gemini_part_function_call_roundtrip() {
        let part = GeminiPart::FunctionCall {
            function_call: GeminiFunctionCall {
                name: "list_files".to_string(),
                args: json!({"path": "/tmp"}),
            },
        };
        let v = serde_json::to_value(&part).unwrap();
        assert!(v.get("functionCall").is_some());
        assert_eq!(v["functionCall"]["name"], "list_files");

        let back: GeminiPart = serde_json::from_value(v).unwrap();
        assert!(matches!(back, GeminiPart::FunctionCall { .. }));
    }

    /// GeminiPart::FunctionResponse serialises with camelCase key.
    #[test]
    fn gemini_part_function_response_roundtrip() {
        let part = GeminiPart::FunctionResponse {
            function_response: GeminiFunctionResponse {
                name: "list_files".to_string(),
                response: json!({"files": ["a.txt"]}),
            },
        };
        let v = serde_json::to_value(&part).unwrap();
        assert!(v.get("functionResponse").is_some());
    }

    /// GeminiGenerationConfig serialises maxOutputTokens and thinkingConfig.
    #[test]
    fn generation_config_serialisation() {
        let cfg = GeminiGenerationConfig {
            max_output_tokens: 8192,
            thinking_config: Some(GeminiThinkingConfig {
                thinking_budget: -1,
            }),
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["maxOutputTokens"], 8192);
        assert_eq!(v["thinkingConfig"]["thinkingBudget"], -1);
    }

    /// When thinking_config is None, the key is omitted (skip_serializing_if).
    #[test]
    fn generation_config_omits_thinking_when_none() {
        let cfg = GeminiGenerationConfig {
            max_output_tokens: 4096,
            thinking_config: None,
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["maxOutputTokens"], 4096);
        assert!(v.get("thinkingConfig").is_none());
    }

    /// GeminiStreamChunk deserialises with camelCase keys.
    #[test]
    fn stream_chunk_deserialisation() {
        let raw = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "hello"}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5
            }
        });
        let chunk: GeminiStreamChunk = serde_json::from_value(raw).unwrap();
        let candidates = chunk.candidates.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].finish_reason.as_deref(), Some("STOP"));
        let usage = chunk.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, 10);
        assert_eq!(usage.candidates_token_count, 5);
    }

    /// GeminiRequest serialises systemInstruction and generationConfig as camelCase.
    #[test]
    fn gemini_request_camel_case_keys() {
        let req = GeminiRequest {
            contents: vec![GeminiContent {
                role: "user".to_string(),
                parts: vec![GeminiPart::Text {
                    text: "hi".to_string(),
                }],
            }],
            system_instruction: Some(GeminiSystemInstruction {
                parts: vec![GeminiPart::Text {
                    text: "You are helpful.".to_string(),
                }],
            }),
            tools: None,
            generation_config: GeminiGenerationConfig {
                max_output_tokens: 1024,
                thinking_config: None,
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(
            v.get("systemInstruction").is_some(),
            "systemInstruction missing"
        );
        assert!(
            v.get("generationConfig").is_some(),
            "generationConfig missing"
        );
        assert!(
            v.get("system_instruction").is_none(),
            "snake_case key leaked"
        );
    }

    /// ThinkingConfig budget values: -1 (dynamic), 0 (off), N (fixed).
    #[test]
    fn thinking_config_budget_values() {
        for budget in [-1_i32, 0, 1024] {
            let cfg = GeminiThinkingConfig {
                thinking_budget: budget,
            };
            let v = serde_json::to_value(&cfg).unwrap();
            assert_eq!(v["thinkingBudget"], budget);
        }
    }
}
