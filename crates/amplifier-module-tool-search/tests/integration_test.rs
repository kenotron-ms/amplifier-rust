//! Integration tests for amplifier-module-tool-search.
//!
//! Tests verify:
//!  - basic match finding with grep tool (content mode)
//!  - empty result when no matches
//!  - glob filter restricts results to matching file types
//!  - files_with_matches output mode (default)
//!  - GlobTool finds files matching a pattern

use amplifier_core::traits::Tool;
use amplifier_module_tool_search::{GlobTool, GrepTool, SearchConfig};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Test 1: basic match finding (content mode)
// ---------------------------------------------------------------------------

/// `grep` finds matches in Rust source files.
///
/// Writes a `.rs` file containing `println!`, then searches for "println"
/// with `output_mode: "content"`. Asserts that the results array is non-empty,
/// each entry has a `content` containing "println" and a `line` > 0.
#[tokio::test]
async fn grep_finds_matches_in_files() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.rs");
    std::fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

    let config = SearchConfig::new(dir.path().to_path_buf());
    let tool = GrepTool::new(config);

    let result = tool
        .execute(json!({ "pattern": "println", "output_mode": "content" }))
        .await
        .unwrap();

    let output = result.output.unwrap();
    let matches: Vec<Value> = serde_json::from_str(output.as_str().unwrap())
        .expect("output should be a JSON array string");

    assert!(!matches.is_empty(), "expected at least one match");
    let first = &matches[0];
    assert!(
        first["content"].as_str().unwrap_or("").contains("println"),
        "content should contain 'println', got: {}",
        first["content"]
    );
    assert!(
        first["line"].as_u64().unwrap_or(0) > 0,
        "line number should be > 0"
    );
}

// ---------------------------------------------------------------------------
// Test 2: no matches returns empty array
// ---------------------------------------------------------------------------

/// `grep` returns an empty JSON array when the pattern has no matches.
#[tokio::test]
async fn grep_returns_empty_array_for_no_matches() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.rs");
    std::fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

    let config = SearchConfig::new(dir.path().to_path_buf());
    let tool = GrepTool::new(config);

    let result = tool
        .execute(json!({ "pattern": "xyzzy_not_found", "output_mode": "content" }))
        .await
        .unwrap();

    let output = result.output.unwrap();
    let matches: Vec<Value> = serde_json::from_str(output.as_str().unwrap())
        .expect("output should be a JSON array string");

    assert!(
        matches.is_empty(),
        "expected empty matches array, got {} entries",
        matches.len()
    );
}

// ---------------------------------------------------------------------------
// Test 3: glob filter limits results to matching file type
// ---------------------------------------------------------------------------

/// `grep` with `glob='*.rs'` returns only matches from `.rs` files.
///
/// Creates `code.rs` and `notes.md`, both containing the target text.
/// Asserts that only the `.rs` file appears in results.
#[tokio::test]
async fn grep_glob_filters_file_types() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("code.rs"), "search_target here\n").unwrap();
    std::fs::write(dir.path().join("notes.md"), "search_target also\n").unwrap();

    let config = SearchConfig::new(dir.path().to_path_buf());
    let tool = GrepTool::new(config);

    let result = tool
        .execute(json!({
            "pattern": "search_target",
            "glob": "*.rs",
            "output_mode": "content"
        }))
        .await
        .unwrap();

    let output = result.output.unwrap();
    let matches: Vec<Value> = serde_json::from_str(output.as_str().unwrap())
        .expect("output should be a JSON array string");

    assert_eq!(
        matches.len(),
        1,
        "expected exactly 1 match (only the .rs file)"
    );
    let file = matches[0]["file"].as_str().unwrap_or("");
    assert!(
        file.ends_with(".rs"),
        "expected matched file to end with '.rs', got: {}",
        file
    );
}

// ---------------------------------------------------------------------------
// Test 4: files_with_matches mode (default) returns file paths as strings
// ---------------------------------------------------------------------------

/// The default output_mode `files_with_matches` returns plain file path strings.
#[tokio::test]
async fn grep_files_with_matches_returns_paths() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn helper() {}\n").unwrap();
    std::fs::write(dir.path().join("c.rs"), "// no match here\n").unwrap();

    let config = SearchConfig::new(dir.path().to_path_buf());
    let tool = GrepTool::new(config);

    // Default output_mode is files_with_matches
    let result = tool
        .execute(json!({ "pattern": "fn " }))
        .await
        .unwrap();

    let output = result.output.unwrap();
    let paths: Vec<Value> = serde_json::from_str(output.as_str().unwrap())
        .expect("output should be a JSON array string");

    // a.rs and b.rs both contain "fn " — at least 2 results
    assert!(
        paths.len() >= 2,
        "expected at least 2 matching files, got {}",
        paths.len()
    );
    // Each entry is a plain string (file path), not an object
    for p in &paths {
        assert!(
            p.is_string(),
            "files_with_matches entries should be strings, got: {}",
            p
        );
    }
}

// ---------------------------------------------------------------------------
// Test 5: GlobTool finds files matching a pattern
// ---------------------------------------------------------------------------

/// `glob` returns `.rs` files when given pattern `**/*.rs`.
#[tokio::test]
async fn glob_tool_finds_matching_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn foo() {}\n").unwrap();
    std::fs::write(dir.path().join("notes.md"), "# Notes\n").unwrap();

    let config = SearchConfig::new(dir.path().to_path_buf());
    let tool = GlobTool::new(config);

    let result = tool
        .execute(json!({ "pattern": "**/*.rs" }))
        .await
        .unwrap();

    let output = result.output.unwrap();
    let obj: Value = serde_json::from_str(output.as_str().unwrap())
        .expect("output should be a JSON object string");

    let matches = obj["matches"].as_array().expect("matches should be an array");
    let count = obj["count"].as_u64().expect("count should be an integer");
    let total = obj["total_files"].as_u64().expect("total_files should be an integer");

    assert_eq!(matches.len() as u64, count, "count should equal matches.len()");
    assert!(total >= 2, "expected at least 2 total files, got {}", total);

    for m in matches {
        assert!(
            m.as_str().unwrap_or("").ends_with(".rs"),
            "all matches should be .rs files, got: {}",
            m
        );
    }
}
