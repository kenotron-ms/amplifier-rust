//! Codebase search backend: ripgrep subprocess with pure-Rust fallback.
//!
//! The public entry point [`grep`] tries `rg` first and falls back to the
//! pure-Rust `grep_fallback` implementation on any error (e.g. `rg` not
//! installed).

use std::path::PathBuf;

use regex::RegexBuilder;
use serde_json::{json, Value};
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Controls what grep results look like.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputMode {
    /// Return only matching file paths — one JSON string per entry. **Default.**
    FilesWithMatches,
    /// Return `{file, line, content}` for each matching line (and context lines).
    Content,
    /// Return `{file, count}` per matching file.
    Count,
}

impl Default for OutputMode {
    fn default() -> Self {
        Self::FilesWithMatches
    }
}

/// Full parameter set for a grep search.
#[derive(Debug, Clone)]
pub struct GrepParams {
    pub pattern: String,
    pub path: PathBuf,
    pub glob_filter: Option<String>,
    /// Upper bound on returned entries after applying `offset`.
    pub head_limit: usize,
    /// Number of leading entries to skip.
    pub offset: usize,
    pub case_insensitive: bool,
    /// Lines of context before each match (`-B`).
    pub context_before: usize,
    /// Lines of context after each match (`-A`).
    pub context_after: usize,
    pub output_mode: OutputMode,
    /// Pass `--no-ignore` to rg (include .gitignored / hidden files).
    pub include_ignored: bool,
    /// rg `--type` filter (e.g. `"py"`, `"rs"`, `"ts"`).
    pub file_type: Option<String>,
    /// Enable rg multiline mode (`-U --multiline-dotall`).
    pub multiline: bool,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Search `params.path` for content matching `params.pattern`.
///
/// Tries `rg` first; on any error falls back to the pure-Rust walker.
pub async fn grep(params: &GrepParams) -> Result<Vec<Value>, String> {
    match grep_ripgrep(params).await {
        Ok(results) => Ok(results),
        Err(_) => grep_fallback(params).await,
    }
}

// ---------------------------------------------------------------------------
// ripgrep backend
// ---------------------------------------------------------------------------

async fn grep_ripgrep(params: &GrepParams) -> Result<Vec<Value>, String> {
    let mut cmd = tokio::process::Command::new("rg");

    match params.output_mode {
        OutputMode::FilesWithMatches => {
            cmd.arg("--files-with-matches");
        }
        OutputMode::Count => {
            cmd.arg("--count");
        }
        OutputMode::Content => {
            cmd.arg("--json");
            if params.context_after > 0 {
                cmd.arg("-A");
                cmd.arg(params.context_after.to_string());
            }
            if params.context_before > 0 {
                cmd.arg("-B");
                cmd.arg(params.context_before.to_string());
            }
        }
    }

    if params.case_insensitive {
        cmd.arg("-i");
    }
    if params.include_ignored {
        cmd.arg("--no-ignore");
    }
    if params.multiline {
        cmd.arg("-U");
        cmd.arg("--multiline-dotall");
    }
    if let Some(ref ft) = params.file_type {
        cmd.arg("--type");
        cmd.arg(ft);
    }
    if let Some(ref glob) = params.glob_filter {
        cmd.arg("-g");
        cmd.arg(glob);
    }

    cmd.arg("--");
    cmd.arg(&params.pattern);
    cmd.arg(&params.path);

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
    // Collect up to offset + head_limit entries, then slice.
    let cap = params.offset.saturating_add(params.head_limit);
    let mut raw: Vec<Value> = Vec::new();

    match params.output_mode {
        OutputMode::FilesWithMatches => {
            for line in stdout.lines() {
                if line.is_empty() {
                    continue;
                }
                raw.push(json!(line.replace('\\', "/")));
                if raw.len() >= cap {
                    break;
                }
            }
        }
        OutputMode::Count => {
            // rg --count format per line: "filepath:count"
            for line in stdout.lines() {
                if line.is_empty() {
                    continue;
                }
                if let Some(colon) = line.rfind(':') {
                    let file = line[..colon].replace('\\', "/");
                    if let Ok(count) = line[colon + 1..].parse::<u64>() {
                        raw.push(json!({"file": file, "count": count}));
                    }
                }
                if raw.len() >= cap {
                    break;
                }
            }
        }
        OutputMode::Content => {
            // rg --json NDJSON: include both "match" and "context" type entries.
            for line in stdout.lines() {
                if line.is_empty() {
                    continue;
                }
                let parsed: Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let entry_type = parsed.get("type").and_then(|t| t.as_str());
                if !matches!(entry_type, Some("match") | Some("context")) {
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
                raw.push(json!({"file": file, "line": line_num, "content": content}));
                if raw.len() >= cap {
                    break;
                }
            }
        }
    }

    let start = params.offset.min(raw.len());
    Ok(raw[start..].to_vec())
}

// ---------------------------------------------------------------------------
// Pure-Rust fallback backend
// ---------------------------------------------------------------------------

/// Walk `path` recursively, matching content against `pattern` with the
/// `regex` crate and an optional `glob::Pattern` file-name filter.
///
/// Runs on a blocking thread pool via [`tokio::task::spawn_blocking`].
async fn grep_fallback(params: &GrepParams) -> Result<Vec<Value>, String> {
    let params = params.clone();

    tokio::task::spawn_blocking(move || {
        let mut rb = RegexBuilder::new(&params.pattern);
        rb.case_insensitive(params.case_insensitive);
        if params.multiline {
            rb.multi_line(true).dot_matches_new_line(true);
        }
        let re = rb.build().map_err(|e| format!("Invalid regex: {}", e))?;

        let glob_pat: Option<glob::Pattern> = match &params.glob_filter {
            Some(g) => Some(
                glob::Pattern::new(g)
                    .map_err(|e| format!("Invalid glob pattern: {}", e))?,
            ),
            None => None,
        };

        let type_exts = file_type_extensions(params.file_type.as_deref());
        let cap = params.offset.saturating_add(params.head_limit);
        let include_ignored = params.include_ignored;

        let mut raw: Vec<Value> = Vec::new();
        // Count mode accumulates per-file counts separately.
        let mut file_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();

        let mut walker = WalkDir::new(&params.path)
            .follow_links(false)
            .into_iter();

        'walk: loop {
            let entry = match walker.next() {
                None => break 'walk,
                Some(Err(_)) => continue,
                Some(Ok(e)) => e,
            };

            // Skip hidden entries (names starting with '.') unless include_ignored.
            if !include_ignored {
                let name = entry.file_name().to_str().unwrap_or("");
                if name.starts_with('.') {
                    if entry.file_type().is_dir() {
                        walker.skip_current_dir();
                    }
                    continue;
                }
            }

            if !entry.file_type().is_file() {
                continue;
            }

            let file_path = entry.path();

            // Apply optional glob filename filter.
            if let Some(ref pat) = glob_pat {
                let fname = file_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if !pat.matches(fname) {
                    continue;
                }
            }

            // Apply file-type extension filter.
            if let Some(ref exts) = type_exts {
                let ext = file_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if !exts.iter().any(|x| *x == ext) {
                    continue;
                }
            }

            // Read file — skip unreadable / binary files.
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let file_str = file_path.to_string_lossy().replace('\\', "/");

            match params.output_mode {
                OutputMode::FilesWithMatches => {
                    if re.is_match(&content) {
                        raw.push(json!(file_str));
                        if raw.len() >= cap {
                            break 'walk;
                        }
                    }
                }
                OutputMode::Count => {
                    let count = re.find_iter(&content).count() as u64;
                    if count > 0 {
                        *file_counts.entry(file_str).or_insert(0) += count;
                        if file_counts.len() >= cap {
                            break 'walk;
                        }
                    }
                }
                OutputMode::Content => {
                    let lines: Vec<&str> = content.lines().collect();
                    let match_idxs: Vec<usize> = lines
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| re.is_match(l))
                        .map(|(i, _)| i)
                        .collect();

                    if match_idxs.is_empty() {
                        continue;
                    }

                    let ranges = expand_context_ranges(
                        &match_idxs,
                        lines.len(),
                        params.context_before,
                        params.context_after,
                    );

                    for (s, e) in ranges {
                        for i in s..=e {
                            raw.push(json!({
                                "file": file_str,
                                "line": i + 1,
                                "content": lines[i],
                            }));
                            if raw.len() >= cap {
                                break 'walk;
                            }
                        }
                    }
                }
            }
        }

        // Convert accumulated file_counts to raw for Count mode.
        if matches!(params.output_mode, OutputMode::Count) {
            raw = file_counts
                .into_iter()
                .map(|(file, count)| json!({"file": file, "count": count}))
                .collect();
        }

        let start = params.offset.min(raw.len());
        Ok(raw[start..].to_vec())
    })
    .await
    .map_err(|e| format!("spawn_blocking panicked: {}", e))?
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Expand match line indices to include context lines, merging overlapping
/// or adjacent ranges.
///
/// Returns a list of `(start_idx, end_idx)` inclusive line-index pairs.
fn expand_context_ranges(
    match_idxs: &[usize],
    file_len: usize,
    before: usize,
    after: usize,
) -> Vec<(usize, usize)> {
    if match_idxs.is_empty() || file_len == 0 {
        return vec![];
    }

    let last = file_len - 1;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut cur_start = match_idxs[0].saturating_sub(before);
    let mut cur_end = (match_idxs[0] + after).min(last);

    for &idx in &match_idxs[1..] {
        let r_start = idx.saturating_sub(before);
        let r_end = (idx + after).min(last);
        if r_start <= cur_end + 1 {
            // Overlapping or adjacent — merge.
            cur_end = cur_end.max(r_end);
        } else {
            ranges.push((cur_start, cur_end));
            cur_start = r_start;
            cur_end = r_end;
        }
    }
    ranges.push((cur_start, cur_end));
    ranges
}

/// Map rg file-type aliases to known file extensions for the pure-Rust fallback.
fn file_type_extensions(file_type: Option<&str>) -> Option<Vec<&'static str>> {
    match file_type? {
        "py" | "python" => Some(vec!["py"]),
        "rs" | "rust" => Some(vec!["rs"]),
        "js" | "javascript" => Some(vec!["js", "mjs", "cjs"]),
        "ts" | "typescript" => Some(vec!["ts", "tsx"]),
        "go" => Some(vec!["go"]),
        "java" => Some(vec!["java"]),
        "c" => Some(vec!["c", "h"]),
        "cpp" | "c++" => Some(vec!["cpp", "cc", "cxx", "hpp", "hh"]),
        "rb" | "ruby" => Some(vec!["rb"]),
        "sh" | "bash" => Some(vec!["sh", "bash"]),
        "md" | "markdown" => Some(vec!["md", "markdown"]),
        "json" => Some(vec!["json"]),
        "yaml" => Some(vec!["yaml", "yml"]),
        "toml" => Some(vec!["toml"]),
        "html" => Some(vec!["html", "htm"]),
        "css" => Some(vec!["css"]),
        "xml" => Some(vec!["xml"]),
        "sql" => Some(vec!["sql"]),
        "swift" => Some(vec!["swift"]),
        "lua" => Some(vec!["lua"]),
        "txt" | "text" => Some(vec!["txt"]),
        "php" => Some(vec!["php"]),
        _ => None,
    }
}
