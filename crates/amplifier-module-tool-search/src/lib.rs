//! Codebase search tools for the Amplifier agent framework.
//!
//! This crate provides two tools:
//!
//! - [`GrepTool`] — search file contents with a regex pattern, using ripgrep
//!   (`rg`) when available with a pure-Rust `walkdir`+`regex` fallback.
//! - [`GlobTool`] — find files/directories matching a glob pattern.
//!
//! # Output modes (GrepTool)
//!
//! | `output_mode`        | Result shape                       |
//! |----------------------|------------------------------------|
//! | `files_with_matches` | `["path/to/file", ...]` (default)  |
//! | `content`            | `[{file, line, content}, ...]`     |
//! | `count`              | `[{file, count}, ...]`             |

/// ripgrep-based search backend.
pub mod ripgrep;

/// Glob file-finder tool.
pub mod glob;

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

pub use glob::GlobTool;

// ---------------------------------------------------------------------------
// SearchConfig
// ---------------------------------------------------------------------------

/// Shared configuration for search tools.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Base directory for relative path resolution.
    pub base_path: PathBuf,
    /// Default maximum number of results to return.
    pub max_results: usize,
}

impl SearchConfig {
    /// Create a new [`SearchConfig`] with the given base path.
    ///
    /// Defaults to `max_results = 500`.
    pub fn new(base_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            base_path,
            max_results: 500,
        })
    }
}

// ---------------------------------------------------------------------------
// GrepTool
// ---------------------------------------------------------------------------

/// Tool for searching file contents with a regex pattern.
///
/// Uses ripgrep (`rg`) when available, with a pure-Rust `walkdir`+`regex`
/// fallback. Supports multiple output modes, context lines, file-type
/// filters, case-insensitive search, and multiline patterns.
pub struct GrepTool {
    config: Arc<SearchConfig>,
}

impl GrepTool {
    /// Create a new [`GrepTool`] with the given configuration.
    pub fn new(config: Arc<SearchConfig>) -> Self {
        Self { config }
    }
}

// ---------------------------------------------------------------------------
// Async implementation helper
// ---------------------------------------------------------------------------

async fn execute_grep(config: Arc<SearchConfig>, input: Value) -> Result<ToolResult, ToolError> {
    // Required: pattern
    let pattern = input
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Other {
            message: "missing required parameter: 'pattern'".to_string(),
        })?
        .to_string();

    // Optional: path — absolute or relative to base_path
    let search_path = match input.get("path").and_then(|v| v.as_str()) {
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

    // Optional: glob filter
    let glob_filter = input
        .get("glob")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // head_limit — accept either "head_limit" (primary) or legacy "max_results"
    let head_limit = input
        .get("head_limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(config.max_results);

    // offset — number of leading results to skip
    let offset = input
        .get("offset")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(0);

    // Case insensitive: accept both "-i" and "case_insensitive"
    let case_insensitive = input
        .get("-i")
        .and_then(|v| v.as_bool())
        .or_else(|| input.get("case_insensitive").and_then(|v| v.as_bool()))
        .unwrap_or(false);

    // Context flags: -C overrides -A / -B when both are absent
    let ctx_both = input
        .get("-C")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let context_after = input
        .get("-A")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .or(ctx_both)
        .unwrap_or(0);
    let context_before = input
        .get("-B")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .or(ctx_both)
        .unwrap_or(0);

    // output_mode — default: files_with_matches
    let output_mode = match input.get("output_mode").and_then(|v| v.as_str()) {
        Some("content") => ripgrep::OutputMode::Content,
        Some("count") => ripgrep::OutputMode::Count,
        _ => ripgrep::OutputMode::FilesWithMatches,
    };

    // include_ignored — pass --no-ignore to rg
    let include_ignored = input
        .get("include_ignored")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // type — rg --type filter (e.g. "py", "rs", "ts")
    let file_type = input
        .get("type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // multiline — rg -U --multiline-dotall
    let multiline = input
        .get("multiline")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let params = ripgrep::GrepParams {
        pattern,
        path: search_path,
        glob_filter,
        head_limit,
        offset,
        case_insensitive,
        context_before,
        context_after,
        output_mode,
        include_ignored,
        file_type,
        multiline,
    };

    let results = ripgrep::grep(&params)
        .await
        .map_err(|e| ToolError::Other { message: e })?;

    let json_string = serde_json::to_string(&results).map_err(|e| ToolError::Other {
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

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents using a regex pattern. Uses ripgrep (rg) when \
         available, with a pure-Rust walkdir+regex fallback. Supports multiple \
         output modes, context lines, file-type filters, case-insensitive \
         search, multiline patterns, and result pagination."
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "pattern".to_string(),
            json!({
                "type": "string",
                "description": "The regular expression pattern to search for in file contents"
            }),
        );

        properties.insert(
            "path".to_string(),
            json!({
                "type": "string",
                "description": "File or directory to search in (rg PATH). Defaults to the workspace root."
            }),
        );

        properties.insert(
            "glob".to_string(),
            json!({
                "type": "string",
                "description": "Glob pattern to filter files (e.g. '*.rs', '**/*.tsx') — maps to rg --glob"
            }),
        );

        properties.insert(
            "output_mode".to_string(),
            json!({
                "type": "string",
                "enum": ["files_with_matches", "content", "count"],
                "description": "Output mode: 'files_with_matches' (default) returns file paths, \
                                'content' returns {file, line, content} for each matching line, \
                                'count' returns {file, count} per matching file"
            }),
        );

        properties.insert(
            "-i".to_string(),
            json!({
                "type": "boolean",
                "description": "Case insensitive search (rg -i)"
            }),
        );

        properties.insert(
            "-A".to_string(),
            json!({
                "type": "integer",
                "description": "Number of lines to show after each match (rg -A). Requires output_mode: 'content'."
            }),
        );

        properties.insert(
            "-B".to_string(),
            json!({
                "type": "integer",
                "description": "Number of lines to show before each match (rg -B). Requires output_mode: 'content'."
            }),
        );

        properties.insert(
            "-C".to_string(),
            json!({
                "type": "integer",
                "description": "Lines of context before and after each match (rg -C). Overrides -A and -B. Requires output_mode: 'content'."
            }),
        );

        properties.insert(
            "head_limit".to_string(),
            json!({
                "type": "integer",
                "description": "Limit output to first N entries (default: 500)"
            }),
        );

        properties.insert(
            "offset".to_string(),
            json!({
                "type": "integer",
                "description": "Skip first N entries before applying head_limit (default: 0)"
            }),
        );

        properties.insert(
            "include_ignored".to_string(),
            json!({
                "type": "boolean",
                "description": "Search in normally-excluded directories (hidden files, .gitignore). Maps to rg --no-ignore. Default: false."
            }),
        );

        properties.insert(
            "type".to_string(),
            json!({
                "type": "string",
                "description": "File type to search (rg --type). Valid types: py, js, ts, go, rust, java, c, cpp, rb, sh, md, json, yaml, html, css, xml, php, sql, swift, lua, toml, txt."
            }),
        );

        properties.insert(
            "multiline".to_string(),
            json!({
                "type": "boolean",
                "description": "Enable multiline mode where '.' matches newlines and patterns can span lines (rg -U --multiline-dotall). Default: false."
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
                "Search file contents using a regex pattern. Uses ripgrep with \
                 pure-Rust fallback. Output modes: files_with_matches (default), \
                 content, count. Supports -i, -A, -B, -C, type, multiline, \
                 include_ignored, head_limit, offset."
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
        Box::pin(async move { execute_grep(config, input).await })
    }
}
