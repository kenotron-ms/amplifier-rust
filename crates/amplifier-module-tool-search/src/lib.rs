//! Codebase search tool for the Amplifier agent framework.
//!
//! This crate provides [`GrepCodebaseTool`], which implements the
//! `amplifier_core::traits::Tool` interface for searching file contents with
//! a regex pattern.
//!
//! # Search backends
//!
//! 1. **ripgrep** (`rg`) — fast subprocess-based search using the system `rg`
//!    binary. Parses NDJSON output (`rg --json`).
//! 2. **Pure-Rust fallback** — `walkdir` + `regex` on a blocking thread pool,
//!    used automatically when `rg` is not available.
//!
//! Both backends produce a JSON array of `{file, line, content}` objects.

pub mod ripgrep;

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

// ---------------------------------------------------------------------------
// SearchConfig
// ---------------------------------------------------------------------------

/// Configuration for [`GrepCodebaseTool`].
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Base directory for relative path resolution.
    pub base_path: PathBuf,
    /// Maximum number of search results to return.
    pub max_results: usize,
}

impl SearchConfig {
    /// Create a new [`SearchConfig`] with the given base path.
    ///
    /// Defaults to `max_results = 200`.
    pub fn new(base_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            base_path,
            max_results: 200,
        })
    }
}

// ---------------------------------------------------------------------------
// GrepCodebaseTool
// ---------------------------------------------------------------------------

/// Tool for searching file contents with a regex pattern.
///
/// Uses ripgrep (`rg`) when available, with a pure-Rust `walkdir`+`regex`
/// fallback. Returns a JSON array of `{file, line, content}` objects.
pub struct GrepCodebaseTool {
    config: Arc<SearchConfig>,
}

impl GrepCodebaseTool {
    /// Create a new [`GrepCodebaseTool`] with the given configuration.
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

    // Optional: max_results override
    let max_results = input
        .get("max_results")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(config.max_results);

    let results = ripgrep::grep(&pattern, &search_path, glob_filter.as_deref(), max_results)
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

impl Tool for GrepCodebaseTool {
    fn name(&self) -> &str {
        "grep_codebase"
    }

    fn description(&self) -> &str {
        "Search file contents using a regex pattern. Uses ripgrep (rg) when \
         available, with a pure-Rust walkdir+regex fallback. Returns a JSON \
         array of {file, line, content} objects."
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
                "description": "Absolute path or path relative to the base_path to search within"
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
            "max_results".to_string(),
            json!({
                "type": "integer",
                "description": "Maximum number of results to return (default: 200)"
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["pattern"]));

        ToolSpec {
            name: "grep_codebase".to_string(),
            parameters,
            description: Some(
                "Search file contents using a regex pattern. Uses ripgrep with \
                 pure-Rust fallback. Returns JSON array of {file, line, content}."
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
