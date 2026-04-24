//! Grep file content search tool implementation.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use amplifier_core::errors::ToolError;
use amplifier_core::messages::ToolSpec;
use amplifier_core::models::ToolResult;
use amplifier_core::traits::Tool;
use regex::Regex;
use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::FilesystemConfig;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of matches to include in the results array.
const MAX_RESULTS: usize = 200;

// ---------------------------------------------------------------------------
// GrepTool
// ---------------------------------------------------------------------------

/// Tool for searching file contents using regex patterns within the vault root.
///
/// Returns a JSON object with a `matches` array. Each match entry contains:
/// - `file`: vault-relative path (forward slashes)
/// - `line`: 1-based line number
/// - `content`: the matching line text
///
/// When the total match count exceeds 200, the response also includes:
/// - `total_matches`: total number of matches found (before truncation)
/// - `truncated`: `true`
pub struct GrepTool {
    config: Arc<FilesystemConfig>,
}

impl GrepTool {
    /// Create a new `GrepTool` with the given filesystem config.
    pub fn new(config: Arc<FilesystemConfig>) -> Self {
        Self { config }
    }
}

// ---------------------------------------------------------------------------
// Async implementation helper
// ---------------------------------------------------------------------------

async fn grep_impl(
    config: Arc<FilesystemConfig>,
    input: Value,
) -> Result<ToolResult, ToolError> {
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

    // Extract optional `glob` (filename glob filter).
    let glob_opt = input
        .get("glob")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract optional context line parameters (not used in result output yet,
    // but declared in the tool spec).
    let _context_before = input
        .get("-B")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let _context_after = input
        .get("-A")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let _context_around = input
        .get("-C")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    // Validate the regex pattern eagerly before offloading.
    Regex::new(&pattern).map_err(|e| ToolError::ExecutionFailed {
        message: format!("Invalid regex: {}", e),
        stdout: None,
        stderr: None,
        exit_code: None,
    })?;

    let vault_root = config.vault_root.clone();

    // Resolve the search root.
    let search_root = match path_opt {
        Some(ref p) => vault_root.join(p),
        None => vault_root.clone(),
    };

    // Offload the synchronous filesystem walk to a blocking thread.
    let vault_root_for_task = vault_root.clone();
    let (results, total_count) = tokio::task::spawn_blocking(move || {
        // Re-compile regex inside the blocking task (Regex is Send but not Clone).
        let re = Regex::new(&pattern).expect("regex was validated above");

        let mut results: Vec<Value> = Vec::new();
        let mut total_count: usize = 0;

        for entry in WalkDir::new(&search_root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let file_path = entry.path();

            // Apply glob filename filter if specified.
            if let Some(ref glob_pat) = glob_opt {
                let filename = file_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                match glob::Pattern::new(glob_pat) {
                    Ok(pat) => {
                        if !pat.matches(filename) {
                            continue;
                        }
                    }
                    Err(_) => continue,
                }
            }

            // Read file contents — skip binary/unreadable files.
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Compute vault-relative path with forward slashes.
            let rel_path = file_path
                .strip_prefix(&vault_root_for_task)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| file_path.to_string_lossy().replace('\\', "/"));

            // Scan each line for a match.
            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    total_count += 1;
                    if results.len() < MAX_RESULTS {
                        results.push(json!({
                            "file": rel_path,
                            "line": i + 1,
                            "content": line,
                        }));
                    }
                }
            }
        }

        (results, total_count)
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("spawn_blocking failed: {}", e),
        stdout: None,
        stderr: None,
        exit_code: None,
    })?;

    // Build the output object.
    let mut output = serde_json::Map::new();
    output.insert("matches".to_string(), Value::Array(results));

    if total_count > MAX_RESULTS {
        output.insert(
            "total_matches".to_string(),
            json!(total_count as u64),
        );
        output.insert("truncated".to_string(), json!(true));
    }

    Ok(ToolResult {
        success: true,
        output: Some(Value::Object(output)),
        error: None,
    })
}

// ---------------------------------------------------------------------------
// Tool impl
// ---------------------------------------------------------------------------

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents using regex patterns within the vault root. \
         Returns a JSON object with a `matches` array (up to 200 matches). \
         When the total exceeds 200, `total_matches` and `truncated: true` are also included. \
         Each match contains: `file` (vault-relative path), `line` (1-based), `content` (line text)."
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "pattern".to_string(),
            json!({
                "type": "string",
                "description": "Regex pattern to search for in file contents"
            }),
        );

        properties.insert(
            "path".to_string(),
            json!({
                "type": "string",
                "description": "Optional subdirectory relative to the vault root to search within"
            }),
        );

        properties.insert(
            "glob".to_string(),
            json!({
                "type": "string",
                "description": "Optional filename glob filter (e.g. '*.rs') to restrict which files are searched"
            }),
        );

        properties.insert(
            "-A".to_string(),
            json!({
                "type": "integer",
                "description": "Number of context lines to show after each match"
            }),
        );

        properties.insert(
            "-B".to_string(),
            json!({
                "type": "integer",
                "description": "Number of context lines to show before each match"
            }),
        );

        properties.insert(
            "-C".to_string(),
            json!({
                "type": "integer",
                "description": "Number of context lines to show before and after each match"
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["pattern"]));

        ToolSpec {
            name: "grep".to_string(),
            parameters,
            description: Some(
                "Search file contents using regex patterns. Returns up to 200 matches; \
                 when truncated, total_matches and truncated:true are included."
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
        Box::pin(async move { grep_impl(config, input).await })
    }
}
