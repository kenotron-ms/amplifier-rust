//! Session store — persistence layer for agent session events and metadata.
//!
//! Provides [`SessionStore`], a trait for persisting session events,
//! and [`FileSessionStore`], a file-based implementation.

/// File-based session store implementation.
pub mod file;
/// Session data format types (events, metadata, index entries).
pub mod format;

pub use file::FileSessionStore;
pub use format::{IndexEntry, SessionEvent, SessionMetadata};

use async_trait::async_trait;

/// Convenience alias so callers and implementors don't need to import `anyhow` directly.
pub type Result<T> = anyhow::Result<T>;

/// Trait for persisting and loading session data.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Begin a new session, recording initial metadata.
    async fn begin(&self, session_id: &str, metadata: SessionMetadata) -> Result<()>;

    /// Append an event to an existing session log.
    async fn append(&self, session_id: &str, event: SessionEvent) -> Result<()>;

    /// Mark a session as finished with a final status and turn count.
    async fn finish(&self, session_id: &str, status: &str, turn_count: usize) -> Result<()>;

    /// Load all events recorded for a session.
    async fn load(&self, session_id: &str) -> Result<Vec<SessionEvent>>;

    /// List metadata for all known sessions.
    async fn list(&self) -> Result<Vec<SessionMetadata>>;

    /// Return true if a session with the given ID already exists.
    async fn exists(&self, session_id: &str) -> bool;
}
