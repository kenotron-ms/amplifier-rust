//! Glob file pattern matching tool implementation.

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
// Pipe helper trait
// ---------------------------------------------------------------------------

/// A helper trait that allows chaining a value into a function call.
///
/// Enables patterns like `value.pipe(Ok::<_, String>)` to lift a value into
/// a `Result` without needing an intermediate binding.
trait Pipe: Sized {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R,
    {
        f(self)
    }
}

impl<T> Pipe for T {}

// ---------------------------------------------------------------------------
// GlobTool
// ---------------------------------------------------------------------------

/// Tool for matching files using glob patterns within the vault root.
///
/// Returns a JSON array of vault-relative path strings (forward-slash separated,
/// no leading `/`).
pub struct GlobTool {
    config: Arc<FilesystemConfig>,
}

impl GlobTool {
    /// Create a new `GlobTool` with the given filesystem config.
    pub fn new(config: Arc<FilesystemConfig>) -> Self {
        Self { config }
    }
}

// ---------------------------------------------------------------------------
// Async implementation helper
// ---------------------------------------------------------------------------

async fn glob_impl(config: Arc<FilesystemConfig>, input: Value) -> Result<ToolResult, ToolError> {
    // Extract required `pattern` parameter.
    let pattern = input
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Other {
            message: "missing required parameter: 'pattern'".to_string(),
        })?
        .to_string();

    // Extract optional `path` (subdirectory relative to vault root).
    let path_opt = input
        .get("path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let vault_root = config.vault_root.clone();

    // Resolve the base directory.
    let base = match path_opt {
        Some(ref p) => vault_root.join(p),
        None => vault_root.clone(),
    };

    // Build the full glob pattern string.
    let full_pattern = base.join(&pattern);
    let pattern_str = full_pattern.to_string_lossy().into_owned();

    // Run the synchronous glob::glob() inside spawn_blocking.
    let vault_root_for_task = vault_root.clone();
    let paths = tokio::task::spawn_blocking(move || {
        let entries = glob::glob(&pattern_str).map_err(|e| ToolError::ExecutionFailed {
            message: format!("Invalid glob pattern: {}", e),
            stdout: None,
            stderr: None,
            exit_code: None,
        })?;

        let mut results: Vec<String> = Vec::new();
        for entry in entries.filter_map(|e| e.ok()) {
            if let Ok(rel) = entry.strip_prefix(&vault_root_for_task) {
                // Normalize path separators to forward slashes for cross-platform consistency.
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                results.push(rel_str);
            }
        }
        results.pipe(Ok::<_, ToolError>)
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("spawn_blocking failed: {}", e),
        stdout: None,
        stderr: None,
        exit_code: None,
    })??;

    let output = serde_json::to_string(&paths).map_err(|e| ToolError::ExecutionFailed {
        message: format!("failed to serialize glob results: {}", e),
        stdout: None,
        stderr: None,
        exit_code: None,
    })?;

    Ok(ToolResult {
        success: true,
        output: Some(json!(output)),
        error: None,
    })
}

// ---------------------------------------------------------------------------
// Tool impl
// ---------------------------------------------------------------------------

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Match files using glob patterns within the vault root. \
         Returns a JSON array of vault-relative path strings (e.g. [\"src/main.rs\", \"lib.rs\"]). \
         Paths use forward slashes and have no leading '/'."
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "pattern".to_string(),
            json!({
                "type": "string",
                "description": "Glob pattern to match files (e.g. '**/*.rs')"
            }),
        );

        properties.insert(
            "path".to_string(),
            json!({
                "type": "string",
                "description": "Optional subdirectory relative to the vault root to search within"
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["pattern"]));

        ToolSpec {
            name: "glob".to_string(),
            parameters,
            description: Some(
                "Match files using glob patterns within the vault root. \
                 Returns a JSON array of relative path strings."
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
        Box::pin(async move { glob_impl(config, input).await })
    }
}
