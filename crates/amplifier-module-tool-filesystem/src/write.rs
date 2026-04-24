//! Write and edit file tool implementations.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use amplifier_core::errors::ToolError;
use amplifier_core::messages::ToolSpec;
use amplifier_core::models::ToolResult;
use amplifier_core::traits::Tool;
use serde_json::{json, Value};

use crate::FilesystemConfig;

// ---------------------------------------------------------------------------
// WriteFileTool
// ---------------------------------------------------------------------------

/// Tool for writing file contents within allowed write paths.
pub struct WriteFileTool {
    config: Arc<FilesystemConfig>,
}

impl WriteFileTool {
    /// Create a new `WriteFileTool` with the given filesystem config.
    pub fn new(config: Arc<FilesystemConfig>) -> Self {
        Self { config }
    }
}

// ---------------------------------------------------------------------------
// EditFileTool
// ---------------------------------------------------------------------------

/// Tool for editing file contents within allowed write paths.
pub struct EditFileTool {
    config: Arc<FilesystemConfig>,
}

impl EditFileTool {
    /// Create a new `EditFileTool` with the given filesystem config.
    pub fn new(config: Arc<FilesystemConfig>) -> Self {
        Self { config }
    }
}

// ---------------------------------------------------------------------------
// Helper: nearest existing ancestor
// ---------------------------------------------------------------------------

/// Walk up the path until an existing entry is found.
/// Returns `None` if no ancestor exists (e.g., root is unreachable).
fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return Some(current);
        }
        let parent = current.parent()?.to_path_buf();
        current = parent;
    }
}

/// Count non-overlapping occurrences of `needle` in `haystack`.
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        count += 1;
        start += pos + needle.len();
    }
    count
}

// ---------------------------------------------------------------------------
// WriteFileTool async implementation
// ---------------------------------------------------------------------------

async fn write_file_impl(
    config: Arc<FilesystemConfig>,
    input: Value,
) -> Result<ToolResult, ToolError> {
    // Extract required parameters.
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Other {
            message: "missing required parameter: 'path'".to_string(),
        })?;

    let content_str = input
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Other {
            message: "missing required parameter: 'content'".to_string(),
        })?;

    // Resolve absolute path.
    let abs_path = config.vault_root.join(path_str);

    // Vault boundary check: canonicalize vault_root and nearest existing ancestor of abs_path.
    let canonical_root =
        tokio::fs::canonicalize(&config.vault_root)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("failed to canonicalize vault root: {}", e),
                stdout: None,
                stderr: None,
                exit_code: None,
            })?;

    let ancestor = nearest_existing_ancestor(&abs_path).ok_or_else(|| ToolError::ExecutionFailed {
        message: "Write access denied: outside vault root".to_string(),
        stdout: None,
        stderr: None,
        exit_code: None,
    })?;

    let canonical_check =
        tokio::fs::canonicalize(&ancestor)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("failed to canonicalize path: {}", e),
                stdout: None,
                stderr: None,
                exit_code: None,
            })?;

    if !canonical_check.starts_with(&canonical_root) {
        return Err(ToolError::ExecutionFailed {
            message: "Write access denied: outside vault root".to_string(),
            stdout: None,
            stderr: None,
            exit_code: None,
        });
    }

    // Allowed write paths check.
    if !config
        .allowed_write_paths
        .iter()
        .any(|p| abs_path.starts_with(p))
    {
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "Write access denied: '{}' is not in allowed write paths",
                path_str
            ),
            stdout: None,
            stderr: None,
            exit_code: None,
        });
    }

    // Create parent directories.
    if let Some(parent) = abs_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("failed to create parent directories: {}", e),
                stdout: None,
                stderr: None,
                exit_code: None,
            })?;
    }

    // Write the file.
    let bytes = content_str.as_bytes();
    tokio::fs::write(&abs_path, bytes)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("failed to write '{}': {}", path_str, e),
            stdout: None,
            stderr: None,
            exit_code: None,
        })?;

    Ok(ToolResult {
        success: true,
        output: Some(json!(format!("Wrote {} bytes to {}", bytes.len(), path_str))),
        error: None,
    })
}

// ---------------------------------------------------------------------------
// EditFileTool async implementation
// ---------------------------------------------------------------------------

async fn edit_file_impl(
    config: Arc<FilesystemConfig>,
    input: Value,
) -> Result<ToolResult, ToolError> {
    // Extract required parameters.
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Other {
            message: "missing required parameter: 'path'".to_string(),
        })?;

    let old_string = input
        .get("old_string")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Other {
            message: "missing required parameter: 'old_string'".to_string(),
        })?;

    let new_string = input
        .get("new_string")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Other {
            message: "missing required parameter: 'new_string'".to_string(),
        })?;

    let replace_all = input
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Resolve absolute path.
    let abs_path = config.vault_root.join(path_str);

    // Allowed write paths check.
    if !config
        .allowed_write_paths
        .iter()
        .any(|p| abs_path.starts_with(p))
    {
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "Write access denied: '{}' is not in allowed write paths",
                path_str
            ),
            stdout: None,
            stderr: None,
            exit_code: None,
        });
    }

    // Read the file.
    let content = tokio::fs::read_to_string(&abs_path)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("failed to read '{}': {}", path_str, e),
            stdout: None,
            stderr: None,
            exit_code: None,
        })?;

    // Check old_string is present.
    if !content.contains(old_string) {
        return Err(ToolError::ExecutionFailed {
            message: format!("old_string not found in '{}'", path_str),
            stdout: None,
            stderr: None,
            exit_code: None,
        });
    }

    // Replace occurrences.
    let (new_content, count) = if replace_all {
        let n = count_occurrences(&content, old_string);
        (content.replace(old_string, new_string), n)
    } else {
        (content.replacen(old_string, new_string, 1), 1)
    };

    // Write back.
    tokio::fs::write(&abs_path, new_content.as_bytes())
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("failed to write '{}': {}", path_str, e),
            stdout: None,
            stderr: None,
            exit_code: None,
        })?;

    Ok(ToolResult {
        success: true,
        output: Some(json!(format!(
            "Replaced {} occurrence(s) in {}",
            count, path_str
        ))),
        error: None,
    })
}

// ---------------------------------------------------------------------------
// Tool impl for WriteFileTool
// ---------------------------------------------------------------------------

impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file within allowed write paths. Creates parent directories as needed."
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "path".to_string(),
            json!({
                "type": "string",
                "description": "Path to the file relative to the vault root"
            }),
        );

        properties.insert(
            "content".to_string(),
            json!({
                "type": "string",
                "description": "Content to write to the file"
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["path", "content"]));

        ToolSpec {
            name: "write_file".to_string(),
            parameters,
            description: Some(
                "Write content to a file within allowed write paths. \
                 Creates parent directories as needed."
                    .to_string(),
            ),
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        let config = Arc::clone(&self.config);
        Box::pin(async move { write_file_impl(config, input).await })
    }
}

// ---------------------------------------------------------------------------
// Tool impl for EditFileTool
// ---------------------------------------------------------------------------

impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit file contents by replacing occurrences of a string within allowed write paths."
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "path".to_string(),
            json!({
                "type": "string",
                "description": "Path to the file relative to the vault root"
            }),
        );

        properties.insert(
            "old_string".to_string(),
            json!({
                "type": "string",
                "description": "String to find and replace in the file"
            }),
        );

        properties.insert(
            "new_string".to_string(),
            json!({
                "type": "string",
                "description": "Replacement string"
            }),
        );

        properties.insert(
            "replace_all".to_string(),
            json!({
                "type": "boolean",
                "description": "Replace all occurrences instead of just the first (default: false)"
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert(
            "required".to_string(),
            json!(["path", "old_string", "new_string"]),
        );

        ToolSpec {
            name: "edit_file".to_string(),
            parameters,
            description: Some(
                "Edit file contents by replacing occurrences of a string. \
                 Replaces only the first occurrence by default."
                    .to_string(),
            ),
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        let config = Arc::clone(&self.config);
        Box::pin(async move { edit_file_impl(config, input).await })
    }
}
