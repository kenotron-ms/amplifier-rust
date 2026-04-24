//! Integration tests for OpenAI Responses API provider.

use amplifier_core::messages::{ChatRequest, ContentBlock, Message, MessageContent, Role};
use amplifier_core::traits::Provider;
use amplifier_module_provider_openai::{OpenAIConfig, OpenAIProvider};
use serde_json::json;
use std::collections::HashMap;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Builds a minimal ChatRequest with a single user text message.
fn make_request(user_text: &str) -> ChatRequest {
    ChatRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text(user_text.to_string()),
            name: None,
            tool_call_id: None,
            metadata: None,
            extensions: HashMap::new(),
        }],
        tools: None,
        response_format: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        conversation_id: None,
        stream: None,
        metadata: None,
        model: None,
        tool_choice: None,
        stop: None,
        reasoning_effort: None,
        timeout: None,
        extensions: HashMap::new(),
    }
}

/// Extracts the first text block from a ChatResponse content list.
fn first_text(content: &[ContentBlock]) -> Option<String> {
    content.iter().find_map(|block| {
        if let ContentBlock::Text { text, .. } = block {
            Some(text.clone())
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------
// Test 1: Bearer auth header is sent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_sends_bearer_auth_header() {
    let mock_server = MockServer::start().await;

    // Require Authorization: Bearer test_key — any other header value causes a 404.
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer test_key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "resp_001",
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "Hi there"
                }]
            }],
            "usage": {
                "input_tokens": 5,
                "output_tokens": 3,
                "total_tokens": 8
            },
            "incomplete_details": null,
            "reasoning": null
        })))
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test_key".to_string(),
        model: "gpt-4o".to_string(),
        base_url: mock_server.uri(),
        ..Default::default()
    };

    let provider = OpenAIProvider::new(config);
    let response = provider.complete(make_request("Hello")).await.unwrap();

    assert_eq!(
        first_text(&response.content),
        Some("Hi there".to_string()),
        "response text should be 'Hi there'"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Auto-continuation on max_output_tokens
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_auto_continues_on_max_output_tokens() {
    let mock_server = MockServer::start().await;

    // First call: incomplete, max_output_tokens, partial text "Part one "
    // up_to_n_times(1) is called after respond_with (wiremock 0.6 API)
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "resp_001",
            "status": "incomplete",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "Part one "
                }]
            }],
            "usage": {
                "input_tokens": 5,
                "output_tokens": 3,
                "total_tokens": 8
            },
            "incomplete_details": {
                "reason": "max_output_tokens"
            },
            "reasoning": null
        })))
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // Second call: completed, text "part two"
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "resp_002",
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "part two"
                }]
            }],
            "usage": {
                "input_tokens": 8,
                "output_tokens": 5,
                "total_tokens": 13
            },
            "incomplete_details": null,
            "reasoning": null
        })))
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test_key".to_string(),
        model: "gpt-4o".to_string(),
        base_url: mock_server.uri(),
        ..Default::default()
    };

    let provider = OpenAIProvider::new(config);
    let response = provider.complete(make_request("Hello")).await.unwrap();

    assert_eq!(
        first_text(&response.content),
        Some("Part one part two".to_string()),
        "auto-continued text should be 'Part one part two'"
    );
}
