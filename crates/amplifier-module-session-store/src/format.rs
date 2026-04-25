//! Session data format types.

use serde::{Deserialize, Serialize};

/// One event in a session transcript (one JSONL line in `events.jsonl`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEvent {
    /// Session start marker.
    #[serde(rename = "session_start")]
    SessionStart {
        /// Session identifier.
        session_id: String,
        /// Parent session, if this is a sub-agent session.
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        /// Agent name that owns this session.
        agent_name: String,
        /// ISO-8601 timestamp.
        timestamp: String,
    },
    /// One conversation turn (user or assistant message).
    #[serde(rename = "turn")]
    Turn {
        /// `"user"` or `"assistant"`.
        role: String,
        /// Text content of the turn.
        content: String,
        /// ISO-8601 timestamp.
        timestamp: String,
    },
    /// A single tool invocation and its result.
    #[serde(rename = "tool_call")]
    ToolCall {
        /// Tool name.
        tool: String,
        /// Tool arguments as JSON.
        args: serde_json::Value,
        /// Tool result text.
        result: String,
        /// ISO-8601 timestamp.
        timestamp: String,
    },
    /// Session end marker.
    #[serde(rename = "session_end")]
    SessionEnd {
        /// `"success"` or `"error"`.
        status: String,
        /// Number of agent turns completed.
        turn_count: usize,
        /// ISO-8601 timestamp.
        timestamp: String,
    },
}

/// Metadata describing a session (produced from `session_start` event,
/// surfaced by `SessionStore::list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Unique session identifier.
    pub session_id: String,
    /// Agent name that owns this session.
    pub agent_name: String,
    /// Parent session, if this is a sub-agent session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// ISO-8601 creation timestamp.
    pub created: String,
    /// `"active"` while running, then `"success"` or `"error"`.
    pub status: String,
}

/// One line in the global `~/.amplifier/sessions/index.jsonl` file.
pub type IndexEntry = SessionMetadata;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_start_round_trip() {
        let evt = SessionEvent::SessionStart {
            session_id: "abc".into(),
            parent_id: Some("root".into()),
            agent_name: "explorer".into(),
            timestamp: "2026-04-24T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("\"type\":\"session_start\""), "got {json}");
        assert!(json.contains("\"session_id\":\"abc\""));
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, evt);
    }

    #[test]
    fn turn_round_trip() {
        let evt = SessionEvent::Turn {
            role: "user".into(),
            content: "hello".into(),
            timestamp: "t".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("\"type\":\"turn\""), "got {json}");
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, evt);
    }

    #[test]
    fn tool_call_round_trip() {
        let evt = SessionEvent::ToolCall {
            tool: "bash".into(),
            args: serde_json::json!({"cmd": "ls"}),
            result: "out".into(),
            timestamp: "t".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("\"type\":\"tool_call\""));
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, evt);
    }

    #[test]
    fn session_end_round_trip() {
        let evt = SessionEvent::SessionEnd {
            status: "success".into(),
            turn_count: 3,
            timestamp: "t".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("\"type\":\"session_end\""));
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, evt);
    }

    #[test]
    fn index_entry_round_trip() {
        let entry = IndexEntry {
            session_id: "s".into(),
            agent_name: "a".into(),
            parent_id: None,
            created: "t".into(),
            status: "active".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: IndexEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }
}
