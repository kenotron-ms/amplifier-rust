//! Vault-root-scoped filesystem tools for the Amplifier agent framework.
//!
//! This crate provides the following tools:
//! - [`ReadFileTool`]: Read file contents within the vault root
//! - [`WriteFileTool`]: Write file contents within allowed write paths
//! - [`EditFileTool`]: Edit file contents within allowed write paths
//! - [`GlobTool`]: Match files using glob patterns
//! - [`GrepTool`]: Search file contents using regex patterns

/// Glob pattern file-matching tool.
pub mod glob_tool;
/// Regex-based file content search tool.
pub mod grep_tool;
/// File read tool.
pub mod read;
/// File write and edit tools.
pub mod write;

pub use glob_tool::GlobTool;
pub use grep_tool::GrepTool;
pub use read::ReadFileTool;
pub use write::{EditFileTool, WriteFileTool};

use std::path::PathBuf;
use std::sync::Arc;

/// Configuration for filesystem tools, scoping all operations to a vault root.
#[derive(Debug, Clone)]
pub struct FilesystemConfig {
    /// The root directory that all filesystem operations are scoped to.
    pub vault_root: PathBuf,
    /// Paths where write operations are permitted. Defaults to `[vault_root]`.
    pub allowed_write_paths: Vec<PathBuf>,
    /// Paths where read operations are permitted. `None` means all paths under `vault_root`.
    pub allowed_read_paths: Option<Vec<PathBuf>>,
}

impl FilesystemConfig {
    /// Create a new `FilesystemConfig` with the given vault root.
    ///
    /// By default:
    /// - `allowed_write_paths` is set to `[vault_root.clone()]`
    /// - `allowed_read_paths` is `None` (all paths under vault root are readable)
    pub fn new(vault_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            allowed_write_paths: vec![vault_root.clone()],
            vault_root,
            allowed_read_paths: None,
        })
    }
}
