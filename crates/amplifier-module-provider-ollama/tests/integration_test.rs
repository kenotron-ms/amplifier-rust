//! Integration tests for the Ollama/ChatCompletions-compatible provider.

use amplifier_core::messages::{ChatRequest, ContentBlock, Message, MessageContent, Role};
use amplifier_core::traits::Provider;
use amplifier_module_provider_ollama::{OllamaConfig, OllamaProvider};
use serde_json::json;
use std::collections::HashMap;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a minimal ChatRequest with a single user text message.
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
// Test 1: Sends to /v1/chat/completions and returns text response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ollama_sends_to_chat_completions_endpoint() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello from Ollama"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 8,
                "completion_tokens": 4,
                "total_tokens": 12
            }
        })))
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        api_key: None,
        model: "llama3.2".to_string(),
        max_tokens: 512,
        max_retries: 1,
    };

    let provider = OllamaProvider::new(config);
    let response = provider.complete(make_request("Hi")).await.unwrap();

    assert_eq!(
        first_text(&response.content),
        Some("Hello from Ollama".to_string()),
        "response text should be 'Hello from Ollama'"
    );

    let usage = response.usage.expect("usage should be present");
    assert_eq!(usage.input_tokens, 8, "input_tokens should be 8");
    assert_eq!(usage.output_tokens, 4, "output_tokens should be 4");
}

// ---------------------------------------------------------------------------
// Test 2: Tool call is parsed correctly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ollama_tool_call_is_parsed() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-2",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "fetch_url",
                            "arguments": "{\"url\":\"https://example.com\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        })))
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        api_key: None,
        model: "llama3.2".to_string(),
        max_tokens: 512,
        max_retries: 1,
    };

    let provider = OllamaProvider::new(config);
    let response = provider.complete(make_request("Fetch a URL")).await.unwrap();

    // Find the ToolCall block
    let tool_call_block = response.content.iter().find_map(|block| {
        if let ContentBlock::ToolCall { id, name, input, .. } = block {
            Some((id.clone(), name.clone(), input.clone()))
        } else {
            None
        }
    });

    let (id, name, input) = tool_call_block.expect("expected a ToolCall content block");
    assert_eq!(id, "call_abc", "tool call id should be 'call_abc'");
    assert_eq!(name, "fetch_url", "tool call name should be 'fetch_url'");
    assert_eq!(
        input.get("url").and_then(|v| v.as_str()),
        Some("https://example.com"),
        "tool call input['url'] should be 'https://example.com'"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Real server (ignored by default)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires local Ollama on :11434"]
async fn ollama_real_server_completion() {
    let config = OllamaConfig {
        base_url: "http://localhost:11434".to_string(),
        api_key: None,
        model: "llama3.2".to_string(),
        max_tokens: 512,
        max_retries: 1,
    };

    let provider = OllamaProvider::new(config);
    let response = provider.complete(make_request("Say hello in one word.")).await.unwrap();
    assert!(!response.content.is_empty(), "response should have content");
}
