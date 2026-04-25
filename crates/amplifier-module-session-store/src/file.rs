//! File-based [`SessionStore`] implementation.
//!
//! Each session is stored under `{root}/{session_id}/events.jsonl`.
//! A global `{root}/index.jsonl` holds one `IndexEntry` per line.

use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::format::{IndexEntry, SessionEvent, SessionMetadata};
use crate::{Result, SessionStore};

/// Disk-backed [`SessionStore`] that writes JSONL transcripts under
/// `{root}/{session_id}/events.jsonl` and a global `{root}/index.jsonl`.
pub struct FileSessionStore {
    /// Root directory for all sessions.
    root: PathBuf,
    /// Serializes concurrent writes to the shared `index.jsonl`.
    index_lock: Mutex<()>,
}

impl FileSessionStore {
    /// Create a store rooted at `~/.amplifier/sessions/`.
    ///
    /// Returns an error if the home directory cannot be determined.
    pub fn new() -> anyhow::Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
        Ok(Self::new_with_root(home.join(".amplifier").join("sessions")))
    }

    /// Create a store rooted at an arbitrary directory (used in tests).
    pub fn new_with_root(root: PathBuf) -> Self {
        Self {
            root,
            index_lock: Mutex::new(()),
        }
    }

    /// Root directory of this store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to a session's directory.
    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(session_id)
    }

    /// Path to a session's events.jsonl file.
    pub fn events_file(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("events.jsonl")
    }

    /// Path to the global index.jsonl file.
    pub fn index_file(&self) -> PathBuf {
        self.root.join("index.jsonl")
    }
}

/// Append a single JSON line to `path`, creating it (and parent dirs) if needed.
fn append_jsonl<T: serde::Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(value)?;
    writeln!(f, "{}", line)?;
    f.flush()?;
    Ok(())
}

#[async_trait]
impl SessionStore for FileSessionStore {
    async fn begin(&self, session_id: &str, metadata: SessionMetadata) -> Result<()> {
        std::fs::create_dir_all(self.session_dir(session_id))?;

        let evt = SessionEvent::SessionStart {
            session_id: session_id.to_string(),
            parent_id: metadata.parent_id.clone(),
            agent_name: metadata.agent_name.clone(),
            timestamp: metadata.created.clone(),
        };
        append_jsonl(&self.events_file(session_id), &evt)?;

        let _guard = self.index_lock.lock().await;
        append_jsonl(&self.index_file(), &metadata)?;
        Ok(())
    }

    async fn append(&self, session_id: &str, event: SessionEvent) -> Result<()> {
        append_jsonl(&self.events_file(session_id), &event)
    }

    async fn exists(&self, session_id: &str) -> bool {
        self.events_file(session_id).is_file()
    }

    async fn finish(&self, session_id: &str, status: &str, turn_count: usize) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let evt = SessionEvent::SessionEnd {
            status: status.to_string(),
            turn_count,
            timestamp: now,
        };
        append_jsonl(&self.events_file(session_id), &evt)?;

        let _guard = self.index_lock.lock().await;
        let idx_path = self.index_file();
        let body = match std::fs::read_to_string(&idx_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let mut out = String::new();
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let mut entry: IndexEntry = serde_json::from_str(line)?;
            if entry.session_id == session_id {
                entry.status = status.to_string();
            }
            out.push_str(&serde_json::to_string(&entry)?);
            out.push('\n');
        }
        std::fs::write(&idx_path, out)?;
        Ok(())
    }

    async fn load(&self, session_id: &str) -> Result<Vec<SessionEvent>> {
        let path = self.events_file(session_id);
        let body = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("could not read {}: {}", path.display(), e))?;
        let mut out = Vec::new();
        for (i, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let evt: SessionEvent = serde_json::from_str(line).map_err(|e| {
                anyhow::anyhow!(
                    "malformed JSONL at {} line {}: {}",
                    path.display(),
                    i + 1,
                    e
                )
            })?;
            out.push(evt);
        }
        Ok(out)
    }

    async fn list(&self) -> Result<Vec<SessionMetadata>> {
        let path = self.index_file();
        let body = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for (i, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let m: SessionMetadata = serde_json::from_str(line)
                .map_err(|e| anyhow::anyhow!("malformed index.jsonl line {}: {}", i + 1, e))?;
            out.push(m);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn meta(id: &str) -> SessionMetadata {
        SessionMetadata {
            session_id: id.into(),
            agent_name: "test-agent".into(),
            parent_id: None,
            created: "2026-04-24T00:00:00Z".into(),
            status: "active".into(),
        }
    }

    #[test]
    fn new_with_root_uses_supplied_path() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new_with_root(tmp.path().to_path_buf());
        assert_eq!(store.root(), tmp.path());
    }

    #[test]
    fn session_dir_and_events_file_paths() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new_with_root(tmp.path().to_path_buf());
        assert!(store.session_dir("abc").starts_with(tmp.path()));
        assert!(store.events_file("abc").ends_with("events.jsonl"));
        assert_eq!(store.index_file(), tmp.path().join("index.jsonl"));
    }

    #[tokio::test]
    async fn begin_creates_session_dir_and_index() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new_with_root(tmp.path().to_path_buf());
        store.begin("s1", meta("s1")).await.unwrap();
        assert!(tmp.path().join("s1").is_dir());
        let events = std::fs::read_to_string(tmp.path().join("s1/events.jsonl")).unwrap();
        assert_eq!(events.lines().count(), 1);
        assert!(events.contains("\"type\":\"session_start\""));
        let idx = std::fs::read_to_string(tmp.path().join("index.jsonl")).unwrap();
        assert_eq!(idx.lines().count(), 1);
        assert!(idx.contains("\"status\":\"active\""));
    }

    #[tokio::test]
    async fn append_adds_event_line() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new_with_root(tmp.path().to_path_buf());
        store.begin("s1", meta("s1")).await.unwrap();
        store
            .append(
                "s1",
                SessionEvent::Turn {
                    role: "user".into(),
                    content: "hi".into(),
                    timestamp: "t".into(),
                },
            )
            .await
            .unwrap();
        let body = std::fs::read_to_string(tmp.path().join("s1/events.jsonl")).unwrap();
        assert_eq!(body.lines().count(), 2);
    }

    #[tokio::test]
    async fn exists_true_after_begin() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new_with_root(tmp.path().to_path_buf());
        assert!(!store.exists("missing").await);
        store.begin("s1", meta("s1")).await.unwrap();
        assert!(store.exists("s1").await);
    }

    #[tokio::test]
    async fn finish_appends_end_and_updates_index() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new_with_root(tmp.path().to_path_buf());
        store.begin("s1", meta("s1")).await.unwrap();
        store.finish("s1", "success", 2).await.unwrap();
        let events = std::fs::read_to_string(tmp.path().join("s1/events.jsonl")).unwrap();
        let last = events.lines().last().unwrap();
        assert!(last.contains("\"type\":\"session_end\""));
        assert!(last.contains("\"status\":\"success\""));
        let idx = std::fs::read_to_string(tmp.path().join("index.jsonl")).unwrap();
        assert_eq!(idx.lines().count(), 1, "must not duplicate entry");
        assert!(idx.contains("\"status\":\"success\""));
    }

    #[tokio::test]
    async fn load_returns_events_in_order() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new_with_root(tmp.path().to_path_buf());
        store.begin("s1", meta("s1")).await.unwrap();
        store
            .append(
                "s1",
                SessionEvent::Turn {
                    role: "user".into(),
                    content: "hi".into(),
                    timestamp: "t".into(),
                },
            )
            .await
            .unwrap();
        store.finish("s1", "success", 1).await.unwrap();
        let events = store.load("s1").await.unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], SessionEvent::SessionStart { .. }));
        assert!(matches!(events[1], SessionEvent::Turn { .. }));
        assert!(matches!(events[2], SessionEvent::SessionEnd { .. }));
    }

    #[tokio::test]
    async fn load_returns_error_on_malformed_jsonl() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new_with_root(tmp.path().to_path_buf());
        std::fs::create_dir_all(tmp.path().join("bad")).unwrap();
        std::fs::write(
            tmp.path().join("bad/events.jsonl"),
            "{\"type\":\"session_start\",\"session_id\":\"bad\",\"agent_name\":\"a\",\"timestamp\":\"t\"}\nNOT JSON\n",
        )
        .unwrap();
        let err = store.load("bad").await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("malformed") || msg.contains("line 2"),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn list_returns_sessions_in_order() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new_with_root(tmp.path().to_path_buf());
        for id in ["s1", "s2", "s3"] {
            store.begin(id, meta(id)).await.unwrap();
        }
        let metas = store.list().await.unwrap();
        let ids: Vec<&str> = metas.iter().map(|m| m.session_id.as_str()).collect();
        assert_eq!(ids, vec!["s1", "s2", "s3"]);
    }
}
