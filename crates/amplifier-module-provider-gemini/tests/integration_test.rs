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
