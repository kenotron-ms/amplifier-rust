// stub
use serde::{Deserialize, Serialize};

/// Metadata recorded when a session begins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
}

/// A single event appended to a session log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub data: serde_json::Value,
}

/// An entry in the session index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub session_id: String,
}
