//! File-based session store implementation.
//!
//! Stores session data as JSONL files on disk.

/// A session store backed by the local filesystem.
///
/// Sessions are stored in a configurable directory as JSONL event logs.
pub struct FileSessionStore;
