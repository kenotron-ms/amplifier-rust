//! Responses API wire types for the OpenAI `/v1/responses` endpoint.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Top-level request body for `POST https://api.openai.com/v1/responses`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesRequest {
    /// Model ID (e.g. `"gpt-4o"`).
    pub model: String,
    /// Input items (messages or function call outputs).
    pub input: Vec<ResponsesInputItem>,
    /// Maximum number of tokens in the response.
    pub max_output_tokens: u32,
    /// Optional system-level instructions prepended to the conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Optional list of tools available to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ResponsesTool>>,
    /// Optional reasoning configuration (for `o*` models).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ResponsesReasoning>,
    /// Optional list of additional output modalities to include.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    /// Optional ID of a previous response for multi-turn continuations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
}

/// An item in the `input` array of a [`ResponsesRequest`].
///
/// Tagged by the `"type"` field in JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesInputItem {
    /// A conversational message.
    #[serde(rename = "message")]
    Message {
        /// `"user"`, `"assistant"`, or `"system"`.
        role: String,
        /// Arbitrary message content (text string or structured blocks).
        content: Value,
    },
    /// The output of a previously requested function/tool call.
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        /// The call ID returned by the model in the prior response.
        call_id: String,
        /// The serialized output of the function.
        output: String,
    },
}

/// A tool definition exposed to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesTool {
    /// Always `"function"`.
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function name.
    pub name: String,
    /// Human-readable description of what the function does.
    pub description: String,
    /// JSON Schema describing the function parameters.
    pub parameters: Value,
}

/// Reasoning configuration for `o*` models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesReasoning {
    /// Reasoning effort: `"low"`, `"medium"`, or `"high"`.
    pub effort: String,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Top-level response body from `POST https://api.openai.com/v1/responses`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesResponse {
    /// Unique response ID.
    pub id: String,
    /// Terminal status: `"completed"`, `"incomplete"`, or `"failed"`.
    pub status: String,
    /// Output items produced by the model.
    pub output: Vec<ResponsesOutputItem>,
    /// Token usage statistics (absent when streaming).
    pub usage: Option<ResponsesUsage>,
    /// Details about why the response is incomplete (when `status == "incomplete"`).
    pub incomplete_details: Option<IncompleteDetails>,
    /// Opaque reasoning state returned by `o*` models.
    pub reasoning: Option<ResponsesReasoningState>,
}

/// An item in the `output` array of a [`ResponsesResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesOutputItem {
    /// Item type discriminator (e.g. `"message"`, `"function_call"`).
    #[serde(rename = "type")]
    pub item_type: String,
    /// Content blocks (present for `"message"` items).
    pub content: Option<Vec<ResponsesContent>>,
    /// Call ID (present for `"function_call"` items).
    pub call_id: Option<String>,
    /// Function name (present for `"function_call"` items).
    pub name: Option<String>,
    /// JSON-encoded arguments (present for `"function_call"` items).
    pub arguments: Option<String>,
}

/// A single content block within a [`ResponsesOutputItem`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesContent {
    /// Content type discriminator; typically `"output_text"`.
    #[serde(rename = "type")]
    pub content_type: String,
    /// The text payload (present for `"output_text"` items).
    pub text: Option<String>,
}

/// Token usage reported in a [`ResponsesResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesUsage {
    /// Tokens consumed by the input.
    pub input_tokens: u32,
    /// Tokens produced in the output.
    pub output_tokens: u32,
    /// Total tokens (`input_tokens + output_tokens`).
    pub total_tokens: u32,
    /// Breakdown of output token categories.
    pub output_tokens_details: Option<OutputTokensDetails>,
}

/// Breakdown of output tokens by category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputTokensDetails {
    /// Tokens used for internal chain-of-thought reasoning (`o*` models).
    pub reasoning_tokens: Option<u32>,
}

/// Details explaining why a response has `status == "incomplete"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncompleteDetails {
    /// `"max_output_tokens"` or `"content_filter"`.
    pub reason: String,
}

/// Opaque reasoning state from `o*` models (enables stateful continuation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesReasoningState {
    /// Base64-encoded encrypted reasoning chain.
    pub encrypted_content: Option<String>,
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn input_item_message_serializes_with_type_tag() {
        let item = ResponsesInputItem::Message {
            role: "user".to_string(),
            content: json!("Hello"),
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["type"], "message");
        assert_eq!(v["role"], "user");
    }

    #[test]
    fn input_item_function_call_output_serializes_with_type_tag() {
        let item = ResponsesInputItem::FunctionCallOutput {
            call_id: "call_123".to_string(),
            output: "result".to_string(),
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["type"], "function_call_output");
        assert_eq!(v["call_id"], "call_123");
    }

    #[test]
    fn responses_tool_type_field_renamed_to_type() {
        let tool = ResponsesTool {
            tool_type: "function".to_string(),
            name: "my_fn".to_string(),
            description: "does stuff".to_string(),
            parameters: json!({"type": "object"}),
        };
        let v = serde_json::to_value(&tool).unwrap();
        assert_eq!(v["type"], "function");
        assert_eq!(v["name"], "my_fn");
        assert!(
            v.get("tool_type").is_none(),
            "field should be renamed to 'type'"
        );
    }

    #[test]
    fn responses_content_type_field_renamed_to_type() {
        let content = ResponsesContent {
            content_type: "output_text".to_string(),
            text: Some("hello".to_string()),
        };
        let v = serde_json::to_value(&content).unwrap();
        assert_eq!(v["type"], "output_text");
        assert!(
            v.get("content_type").is_none(),
            "field should be renamed to 'type'"
        );
    }

    #[test]
    fn responses_response_deserializes_completed() {
        let json_str = r#"{
            "id": "resp_001",
            "status": "completed",
            "output": [],
            "usage": null,
            "incomplete_details": null,
            "reasoning": null
        }"#;
        let resp: ResponsesResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(resp.id, "resp_001");
        assert_eq!(resp.status, "completed");
    }

    #[test]
    fn responses_request_serializes_correctly() {
        let req = ResponsesRequest {
            model: "gpt-4o".to_string(),
            input: vec![],
            max_output_tokens: 1024,
            instructions: Some("Be helpful".to_string()),
            tools: None,
            reasoning: None,
            include: None,
            previous_response_id: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], "gpt-4o");
        assert_eq!(v["max_output_tokens"], 1024);
        assert_eq!(v["instructions"], "Be helpful");
    }
}
