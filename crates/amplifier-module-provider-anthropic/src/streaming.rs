//! SSE (Server-Sent Events) parser for Anthropic streaming responses.
//!
//! Anthropic's streaming API sends JSON events over SSE. This module provides
//! pure functions to extract structured data from individual SSE lines without
//! needing to buffer the entire stream.

use serde_json::Value;

/// Strip the `data: ` SSE prefix if present and return the payload.
fn sse_payload(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let payload = trimmed.strip_prefix("data: ").unwrap_or(trimmed);
    Some(payload)
}

/// Extract text from a single SSE line.
///
/// Returns `Some(text)` only when the line carries a `content_block_delta`
/// event whose `delta.type` is `"text_delta"`. All other lines — including
/// the `[DONE]` sentinel, `input_json_delta` deltas, and unrelated event
/// types — return `None`.
pub fn extract_text_from_sse_line(line: &str) -> Option<String> {
    let payload = sse_payload(line)?;

    // Skip the '[DONE]' sentinel
    if payload == "[DONE]" {
        return None;
    }

    let value: Value = serde_json::from_str(payload).ok()?;

    // Must be type = 'content_block_delta'
    if value.get("type")?.as_str()? != "content_block_delta" {
        return None;
    }

    let delta = value.get("delta")?;

    // Must be delta.type = 'text_delta'
    if delta.get("type")?.as_str()? != "text_delta" {
        return None;
    }

    Some(delta.get("text")?.as_str()?.to_string())
}

/// Extract the stop reason from a single SSE line.
///
/// Returns `Some(stop_reason)` when the line carries a `message_delta` event.
/// Returns `None` for all other event types.
pub fn extract_stop_reason_from_sse_line(line: &str) -> Option<String> {
    let payload = sse_payload(line)?;

    if payload == "[DONE]" {
        return None;
    }

    let value: Value = serde_json::from_str(payload).ok()?;

    // Must be type = 'message_delta'
    if value.get("type")?.as_str()? != "message_delta" {
        return None;
    }

    let delta = value.get("delta")?;
    Some(delta.get("stop_reason")?.as_str()?.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_text_from_sse_line ---

    #[test]
    fn extract_text_from_content_block_delta() {
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        assert_eq!(extract_text_from_sse_line(line), Some("Hello".to_string()));
    }

    #[test]
    fn ignores_non_content_block_delta() {
        let line = r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        assert_eq!(extract_text_from_sse_line(line), None);
    }

    #[test]
    fn ignores_non_text_delta_types_like_input_json_delta() {
        // input_json_delta carries partial tool-call JSON; should be ignored
        let line = r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"loc"}}"#;
        assert_eq!(extract_text_from_sse_line(line), None);
    }

    #[test]
    fn ignores_done_sentinel() {
        assert_eq!(extract_text_from_sse_line("data: [DONE]"), None);
        assert_eq!(extract_text_from_sse_line("[DONE]"), None);
    }

    // --- extract_stop_reason_from_sse_line ---

    #[test]
    fn extract_stop_reason_from_message_delta() {
        let line = r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":42}}"#;
        assert_eq!(
            extract_stop_reason_from_sse_line(line),
            Some("end_turn".to_string())
        );
    }

    #[test]
    fn returns_none_for_other_events() {
        let line = r#"data: {"type":"message_start","message":{"id":"msg_01","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":0}}}"#;
        assert_eq!(extract_stop_reason_from_sse_line(line), None);
    }
}
