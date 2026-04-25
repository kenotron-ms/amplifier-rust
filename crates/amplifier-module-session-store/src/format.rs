//! Session data format types.

use serde::{Deserialize, Serialize};

/// Metadata recorded when a session begins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Unique session identifier.
    pub session_id: String,
}

/// A single event appended to a session log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Arbitrary event payload serialized as JSON.
    pub data: serde_json::Value,
}

/// An entry in the session index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    /// Unique session identifier for the indexed session.
    pub session_id: String,
}
