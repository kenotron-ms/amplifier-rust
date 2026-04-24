//! Read file tool implementation.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use amplifier_core::errors::ToolError;
use amplifier_core::messages::ToolSpec;
use amplifier_core::models::ToolResult;
use amplifier_core::traits::Tool;
use serde_json::{json, Value};

use crate::FilesystemConfig;

// ---------------------------------------------------------------------------
// ReadFileTool
// ---------------------------------------------------------------------------

/// Tool for reading file contents within the vault root.
///
/// Returns line-numbered output in `cat -n` style (right-aligned 4-char
/// line number, tab separator, line text).  Supports optional `offset`
/// (0-based number of lines to skip) and `limit` (max lines to return).
pub struct ReadFileTool {
    config: Arc<FilesystemConfig>,
}

impl ReadFileTool {
    /// Create a new `ReadFileTool` with the given filesystem config.
    pub fn new(config: Arc<FilesystemConfig>) -> Self {
        Self { config }
    }
}

// ---------------------------------------------------------------------------
// Async implementation helper
// ---------------------------------------------------------------------------

/// Core file-reading logic, called from the async `execute` wrapper.
async fn read_file_impl(
    config: Arc<FilesystemConfig>,
    input: Value,
) -> Result<ToolResult, ToolError> {
    // Extract required `path` parameter.
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Other {
            message: "missing required parameter: 'path'".to_string(),
        })?;

    // Extract optional `offset` (0-based) and `limit`.
    let offset = input
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let limit = input
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    // Build the full path by joining vault_root with the provided path.
    let full_path = config.vault_root.join(path_str);

    // Check allowed_read_paths allowlist if configured.
    if let Some(ref allowed_paths) = config.allowed_read_paths {
        let is_allowed = allowed_paths.iter().any(|p| full_path.starts_with(p));
        if !is_allowed {
            return Err(ToolError::ExecutionFailed {
                message: format!("path '{}' is not in allowed read paths", path_str),
                stdout: None,
                stderr: None,
                exit_code: None,
            });
        }
    }

    // Read the file contents asynchronously.
    let contents = tokio::fs::read_to_string(&full_path)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("failed to read '{}': {}", path_str, e),
            stdout: None,
            stderr: None,
            exit_code: None,
        })?;

    // Split into lines (strips trailing newline courtesy of `.lines()`).
    let all_lines: Vec<&str> = contents.lines().collect();

    // Apply offset and limit.
    let start = offset;
    let end = if start >= all_lines.len() {
        start // yields an empty slice below
    } else {
        match limit {
            Some(lim) => (start + lim).min(all_lines.len()),
            None => all_lines.len(),
        }
    };
    let selected_lines = if start < all_lines.len() {
        &all_lines[start..end]
    } else {
        &[]
    };

    // Format lines: 4-char right-aligned line number, tab, line text.
    let mut output = String::new();
    for (i, line) in selected_lines.iter().enumerate() {
        output.push_str(&format!("{:>4}\t{}\n", start + i + 1, line));
    }

    Ok(ToolResult {
        success: true,
        output: Some(json!(output)),
        error: None,
    })
}

// ---------------------------------------------------------------------------
// Tool impl
// ---------------------------------------------------------------------------

impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read file contents within the vault root with line-numbered output. \
         Supports optional `offset` (0-based lines to skip) and `limit` \
         (maximum number of lines to return)."
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
            "offset".to_string(),
            json!({
                "type": "integer",
                "description": "0-based line offset to start reading from (default: 0)"
            }),
        );

        properties.insert(
            "limit".to_string(),
            json!({
                "type": "integer",
                "description": "Maximum number of lines to return (default: all remaining lines)"
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["path"]));

        ToolSpec {
            name: "read_file".to_string(),
            parameters,
            description: Some(
                "Read file contents within the vault root with line-numbered output. \
                 Supports optional offset (0-based) and limit parameters."
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
        Box::pin(async move { read_file_impl(config, input).await })
    }
}
