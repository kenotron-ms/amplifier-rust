//! Codebase search backend: ripgrep subprocess with pure-Rust fallback.
//!
//! Both modes return a JSON array of objects:
//!
//! ```json
//! [{"file": "src/main.rs", "line": 42, "content": "    println!(\"hello\");"}]
//! ```
//!
//! The public entry point [`grep`] tries `rg` first and falls back to the
//! pure-Rust [`grep_fallback`] implementation on any error (e.g. `rg` not
//! installed).

use std::path::Path;

use regex::Regex;
use serde_json::{json, Value};
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Search `path` for lines matching `pattern`.
///
/// - Tries `rg --json` first; on failure falls back to the pure-Rust walker.
/// - `glob_filter`: optional filename glob (e.g. `"*.rs"`)
/// - `max_results`: upper bound on the number of returned entries
///
/// Returns `Vec<Value>` where each entry is `{file, line, content}`.
pub async fn grep(
    pattern: &str,
    path: &Path,
    glob_filter: Option<&str>,
    max_results: usize,
) -> Result<Vec<Value>, String> {
    match grep_ripgrep(pattern, path, glob_filter, max_results).await {
        Ok(results) => Ok(results),
        Err(_) => grep_fallback(pattern, path, glob_filter, max_results).await,
    }
}

// ---------------------------------------------------------------------------
// ripgrep backend
// ---------------------------------------------------------------------------

/// Run `rg --json -- {pattern} {path}` and parse NDJSON output.
///
/// Exit code 1 is treated as "no matches" rather than an error.
async fn grep_ripgrep(
    pattern: &str,
    path: &Path,
    glob_filter: Option<&str>,
    max_results: usize,
) -> Result<Vec<Value>, String> {
    let mut cmd = tokio::process::Command::new("rg");
    cmd.arg("--json");
    cmd.arg("--");
    cmd.arg(pattern);
    cmd.arg(path);

    if let Some(glob) = glob_filter {
        cmd.arg("-g");
        cmd.arg(glob);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to launch rg: {}", e))?;

    // Exit code 1 = no matches — treat as success with empty results.
    // Any other non-zero exit code is an error.
    let exit_code = output.status.code().unwrap_or(-1);
    if exit_code != 0 && exit_code != 1 {
        return Err(format!(
            "rg exited with code {}: {}",
            exit_code,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results: Vec<Value> = Vec::new();

    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }

        let parsed: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if parsed.get("type").and_then(|t| t.as_str()) != Some("match") {
            continue;
        }

        let data = match parsed.get("data") {
            Some(d) => d,
            None => continue,
        };

        let file = data
            .get("path")
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .replace('\\', "/");

        let line_num = data
            .get("line_number")
            .and_then(|l| l.as_u64())
            .unwrap_or(0);

        let content = data
            .get("lines")
            .and_then(|l| l.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim_end_matches('\n')
            .to_string();

        results.push(json!({
            "file": file,
            "line": line_num,
            "content": content,
        }));

        if results.len() >= max_results {
            break;
        }
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Pure-Rust fallback backend
// ---------------------------------------------------------------------------

/// Walk `path` recursively, matching lines against `pattern` with the `regex`
/// crate and an optional `glob::Pattern` file-name filter.
///
/// Runs on a blocking thread pool via [`tokio::task::spawn_blocking`].
async fn grep_fallback(
    pattern: &str,
    path: &Path,
    glob_filter: Option<&str>,
    max_results: usize,
) -> Result<Vec<Value>, String> {
    let pattern = pattern.to_string();
    let path = path.to_path_buf();
    let glob_filter = glob_filter.map(|s| s.to_string());

    tokio::task::spawn_blocking(move || {
        let re = Regex::new(&pattern).map_err(|e| format!("Invalid regex: {}", e))?;

        let glob_pat: Option<glob::Pattern> = match &glob_filter {
            Some(g) => {
                Some(glob::Pattern::new(g).map_err(|e| format!("Invalid glob pattern: {}", e))?)
            }
            None => None,
        };

        let mut results: Vec<Value> = Vec::new();

        'walk: for entry in WalkDir::new(&path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let file_path = entry.path();

            // Apply optional glob filename filter.
            if let Some(ref pat) = glob_pat {
                let filename = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !pat.matches(filename) {
                    continue;
                }
            }

            // Read file — skip unreadable / binary files.
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Normalise path separators.
            let file_str = file_path.to_string_lossy().replace('\\', "/");

            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    results.push(json!({
                        "file": file_str,
                        "line": i + 1,
                        "content": line,
                    }));

                    if results.len() >= max_results {
                        break 'walk;
                    }
                }
            }
        }

        Ok(results)
    })
    .await
    .map_err(|e| format!("spawn_blocking panicked: {}", e))?
}
