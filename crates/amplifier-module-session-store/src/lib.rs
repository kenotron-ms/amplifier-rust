pub mod file;
pub mod format;

pub use file::FileSessionStore;
pub use format::{IndexEntry, SessionEvent, SessionMetadata};

use anyhow::Result;
use async_trait::async_trait;

/// Trait for persisting and loading session data.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Begin a new session, recording initial metadata.
    async fn begin(&self, session_id: &str, metadata: SessionMetadata) -> Result<()>;

    /// Append an event to an existing session log.
    async fn append(&self, session_id: &str, event: SessionEvent) -> Result<()>;

    /// Mark a session as finished with a final status and turn count.
    async fn finish(&self, session_id: &str, status: &str, turn_count: u32) -> Result<()>;

    /// Load all events recorded for a session.
    async fn load(&self, session_id: &str) -> Result<Vec<SessionEvent>>;

    /// List metadata for all known sessions.
    async fn list(&self) -> Result<Vec<SessionMetadata>>;

    /// Return true if a session with the given ID already exists.
    async fn exists(&self, session_id: &str) -> bool;
}
