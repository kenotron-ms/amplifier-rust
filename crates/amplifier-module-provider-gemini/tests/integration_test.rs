use amplifier_module_provider_gemini::parse_sse_line;

/// Parses a text-only SSE chunk and extracts the text content.
#[test]
fn sse_text_chunk_extracts_text() {
    let data = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello world"}]}}]}"#;
    let result = parse_sse_line(data);
    assert!(result.is_some(), "expected Some, got None");
    let (text, calls) = result.unwrap();
    assert_eq!(text, "Hello world");
    assert!(calls.is_empty(), "expected no function calls");
}

/// Parses a functionCall SSE chunk and verifies a synthetic ID is generated with
/// the `gemini_call_` prefix.
#[test]
fn sse_function_call_chunk_generates_synthetic_id() {
    let data = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"search_web","args":{"query":"Rust async"}}}]}}]}"#;
    let result = parse_sse_line(data);
    assert!(result.is_some(), "expected Some, got None");
    let (text, calls) = result.unwrap();
    assert_eq!(text, "", "expected empty text");
    assert_eq!(calls.len(), 1, "expected exactly one function call");
    let call = &calls[0];
    assert_eq!(call.name, "search_web");
    assert!(
        call.id.starts_with("gemini_call_"),
        "expected ID to start with 'gemini_call_', got '{}'",
        call.id
    );
}

/// An empty candidates array returns None.
#[test]
fn sse_empty_candidates_returns_none() {
    let data = r#"{"candidates":[]}"#;
    let result = parse_sse_line(data);
    assert!(result.is_none(), "expected None for empty candidates");
}

/// Non-JSON strings (like '[DONE]' and empty string) return None.
#[test]
fn sse_non_json_returns_none() {
    assert!(
        parse_sse_line("[DONE]").is_none(),
        "expected None for '[DONE]'"
    );
    assert!(
        parse_sse_line("").is_none(),
        "expected None for empty string"
    );
}

/// Parsing the same functionCall chunk twice produces two different synthetic IDs.
#[test]
fn two_synthetic_ids_are_distinct() {
    let data = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"search_web","args":{"query":"Rust async"}}}]}}]}"#;
    let (_, calls1) = parse_sse_line(data).unwrap();
    let (_, calls2) = parse_sse_line(data).unwrap();
    assert_ne!(
        calls1[0].id, calls2[0].id,
        "expected distinct IDs, but got '{}' twice",
        calls1[0].id
    );
}

// ---------------------------------------------------------------------------
// Wiremock integration tests — require a live GeminiProvider + mock HTTP server
// ---------------------------------------------------------------------------

#[cfg(test)]
mod wiremock_tests {
    use amplifier_module_provider_gemini::{GeminiConfig, GeminiProvider};
    use amplifier_core::messages::{ChatRequest, ContentBlock, Message, MessageContent, Role};
    use amplifier_core::traits::Provider;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_request(text: &str) -> ChatRequest {
        ChatRequest {
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text(text.to_string()),
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

    /// GeminiProvider streams a multi-chunk SSE response, concatenates text, and
    /// parses usage metadata.
    #[tokio::test]
    async fn gemini_provider_streams_text_response() {
        let mock_server = MockServer::start().await;

        let chunk1 = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello "}]}}]}"#;
        let chunk2 = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"world"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
        let sse_body = format!("data: {}\n\ndata: {}\n\n", chunk1, chunk2);

        Mock::given(method("POST"))
            .and(path_regex(".*streamGenerateContent.*"))
            .and(query_param("key", "test_key"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
            .mount(&mock_server)
            .await;

        let config = GeminiConfig {
            api_key: "test_key".to_string(),
            base_url: mock_server.uri(),
            thinking_budget: 0,
            max_retries: 1,
            ..GeminiConfig::default()
        };

        let provider = GeminiProvider::new(config);
        let request = make_request("Hello");
        let response = provider.complete(request).await.unwrap();

        // Check concatenated text
        let text = response.content.iter().find_map(|b| {
            if let ContentBlock::Text { text, .. } = b {
                Some(text.clone())
            } else {
                None
            }
        });
        assert_eq!(text.unwrap(), "Hello world");

        // Check usage metadata
        let usage = response.usage.expect("expected usage metadata");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    /// GeminiProvider converts a Gemini functionCall SSE chunk into a
    /// ContentBlock::ToolCall with a synthetic ID and correct name/input.
    #[tokio::test]
    async fn gemini_provider_converts_function_call_to_tool_call() {
        let mock_server = MockServer::start().await;

        let chunk = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"search_web","args":{"query":"test"}}}]},"finishReason":"STOP"}]}"#;
        let sse_body = format!("data: {}\n\n", chunk);

        Mock::given(method("POST"))
            .and(path_regex(".*streamGenerateContent.*"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
            .mount(&mock_server)
            .await;

        let config = GeminiConfig {
            api_key: "any_key".to_string(),
            base_url: mock_server.uri(),
            thinking_budget: 0,
            max_retries: 1,
            ..GeminiConfig::default()
        };

        let provider = GeminiProvider::new(config);
        let request = make_request("Search for test");
        let response = provider.complete(request).await.unwrap();

        // Find the ToolCall content block
        let tool_call = response.content.iter().find_map(|b| {
            if let ContentBlock::ToolCall {
                id, name, input, ..
            } = b
            {
                Some((id.clone(), name.clone(), input.clone()))
            } else {
                None
            }
        });

        let (id, name, input) = tool_call.expect("expected a ToolCall content block");
        assert!(
            id.starts_with("gemini_call_"),
            "expected ID to start with 'gemini_call_', got '{}'",
            id
        );
        assert_eq!(name, "search_web");
        assert_eq!(input["query"], serde_json::json!("test"));
    }
}
