//! Glob file finder tool for the Amplifier agent framework.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use amplifier_core::errors::ToolError;
use amplifier_core::messages::ToolSpec;
use amplifier_core::models::ToolResult;
use amplifier_core::traits::Tool;
use serde_json::{json, Value};

use crate::SearchConfig;

/// Tool for finding files and directories matching a glob pattern.
///
/// Patterns are resolved relative to the configured `base_path` (or the
/// `path` parameter when supplied). Returns matching paths with `count`
/// and `total_files` (before any result cap).
pub struct GlobTool {
    config: Arc<SearchConfig>,
}

impl GlobTool {
    /// Create a new [`GlobTool`] with the given configuration.
    pub fn new(config: Arc<SearchConfig>) -> Self {
        Self { config }
    }
}

// ---------------------------------------------------------------------------
// Async implementation helper
// ---------------------------------------------------------------------------

async fn execute_glob(config: Arc<SearchConfig>, input: Value) -> Result<ToolResult, ToolError> {
    // Required: pattern
    let pattern_str = input
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Other {
            message: "missing required parameter: 'pattern'".to_string(),
        })?
        .to_string();

    // Optional: base path — absolute or relative to config.base_path
    let base_path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => {
            let pb = PathBuf::from(p);
            if pb.is_absolute() {
                pb
            } else {
                config.base_path.join(p)
            }
        }
        None => config.base_path.clone(),
    };

    // Optional: entry type filter ("file" | "dir" | "any"), default "file"
    let type_filter = input
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("file")
        .to_string();

    // Optional: exclude glob patterns
    let exclude_patterns: Vec<String> = input
        .get("exclude")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // Optional: include_ignored (include hidden files / dirs), default false
    let include_ignored = input
        .get("include_ignored")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let limit = config.max_results;

    let (matches, total_files) = tokio::task::spawn_blocking(move || {
            // Form the absolute glob pattern by joining base_path with the
            // user-supplied pattern.
            let full_pattern = base_path.join(&pattern_str).to_string_lossy().into_owned();

            let glob_options = glob::MatchOptions {
                case_sensitive: true,
                require_literal_separator: false,
                // When include_ignored=false, wildcards do not match path
                // components that start with '.', so hidden files/dirs are
                // excluded automatically.
                require_literal_leading_dot: !include_ignored,
            };

            // Compile exclude patterns (ignore invalid ones).
            let exclude_pats: Vec<glob::Pattern> = exclude_patterns
                .iter()
                .filter_map(|p| glob::Pattern::new(p).ok())
                .collect();

            let paths = glob::glob_with(&full_pattern, glob_options)
                .map_err(|e| format!("Invalid glob pattern '{}': {}", full_pattern, e))?;

            let mut matches: Vec<String> = Vec::new();
            let mut total_files: usize = 0;

            for path_result in paths {
                let path = match path_result {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Apply entry-type filter.
                let passes_type = match type_filter.as_str() {
                    "dir" => path.is_dir(),
                    "any" => true,
                    _ => path.is_file(), // "file" is the default
                };
                if !passes_type {
                    continue;
                }

                let path_str = path.to_string_lossy().replace('\\', "/");
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");

                // Apply exclude patterns against both the full path and filename.
                let excluded = exclude_pats
                    .iter()
                    .any(|p| p.matches(&path_str) || p.matches(filename));
                if excluded {
                    continue;
                }

                total_files += 1;

                // Only append to matches up to the limit; keep counting total.
                if matches.len() < limit {
                    matches.push(path_str);
                }
            }

            Ok((matches, total_files))
        })
        .await
        .map_err(|e| ToolError::Other {
            message: format!("spawn_blocking panicked: {}", e),
        })?
        .map_err(|e| ToolError::Other { message: e })?;
    let count = matches.len();

    let output = json!({
        "matches": matches,
        "count": count,
        "total_files": total_files,
    });

    let json_string = serde_json::to_string(&output).map_err(|e| ToolError::Other {
        message: format!("serialization error: {}", e),
    })?;

    Ok(ToolResult {
        success: true,
        output: Some(json!(json_string)),
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
        "Fast file pattern matching tool. Supports glob patterns like '**/*.js' \
         or 'src/**/*.ts'. Returns matching file paths sorted by the filesystem, \
         with count and total_files (before any result cap)."
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "pattern".to_string(),
            json!({
                "type": "string",
                "description": "Glob pattern to match files (e.g. '**/*.py', 'src/**/*.ts', '**/*.{js,ts}')"
            }),
        );

        properties.insert(
            "path".to_string(),
            json!({
                "type": "string",
                "description": "Base directory to search from (default: workspace root)"
            }),
        );

        properties.insert(
            "type".to_string(),
            json!({
                "type": "string",
                "enum": ["file", "dir", "any"],
                "description": "Filter by entry type: 'file' (default), 'dir', or 'any'"
            }),
        );

        properties.insert(
            "exclude".to_string(),
            json!({
                "type": "array",
                "items": {"type": "string"},
                "description": "Glob patterns to exclude from results (e.g. ['*.pyc', 'node_modules', '.git'])"
            }),
        );

        properties.insert(
            "include_ignored".to_string(),
            json!({
                "type": "boolean",
                "description": "Include hidden files and directories (those starting with '.'), default: false"
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
                "Find files and directories matching a glob pattern. \
                 Returns {matches: [...paths], count: N, total_files: N}."
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
        Box::pin(async move { execute_glob(config, input).await })
    }
}
