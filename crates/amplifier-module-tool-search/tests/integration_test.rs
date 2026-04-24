//! Integration tests for amplifier-module-tool-search.
//!
//! Tests verify:
//!  - basic match finding with grep_codebase tool
//!  - empty result when no matches
//!  - glob filter restricts results to matching file types

use amplifier_module_tool_search::{GrepCodebaseTool, SearchConfig};
use amplifier_core::traits::Tool;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Test 1: basic match finding
// ---------------------------------------------------------------------------

/// `grep_codebase` finds matches in Rust source files.
///
/// Writes a `.rs` file containing `println!`, then searches for "println".
/// Asserts that the results array is non-empty, each entry has a `content`
/// containing "println" and a `line` > 0.
#[tokio::test]
async fn grep_codebase_finds_matches_in_files() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.rs");
    std::fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

    let config = SearchConfig::new(dir.path().to_path_buf());
    let tool = GrepCodebaseTool::new(config);

    let result = tool
        .execute(json!({ "pattern": "println" }))
        .await
        .unwrap();

    let output = result.output.unwrap();
    let matches: Vec<Value> = serde_json::from_str(output.as_str().unwrap())
        .expect("output should be a JSON array string");

    assert!(!matches.is_empty(), "expected at least one match");
    let first = &matches[0];
    assert!(
        first["content"]
            .as_str()
            .unwrap_or("")
            .contains("println"),
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

/// `grep_codebase` returns an empty JSON array when the pattern has no matches.
#[tokio::test]
async fn grep_codebase_returns_empty_array_for_no_matches() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.rs");
    std::fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

    let config = SearchConfig::new(dir.path().to_path_buf());
    let tool = GrepCodebaseTool::new(config);

    let result = tool
        .execute(json!({ "pattern": "xyzzy_not_found" }))
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

/// `grep_codebase` with `glob='*.rs'` returns only matches from `.rs` files.
///
/// Creates `code.rs` and `notes.md`, both containing the target text.
/// Asserts that only the `.rs` file appears in results.
#[tokio::test]
async fn grep_codebase_glob_filters_file_types() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("code.rs"), "search_target here\n").unwrap();
    std::fs::write(dir.path().join("notes.md"), "search_target also\n").unwrap();

    let config = SearchConfig::new(dir.path().to_path_buf());
    let tool = GrepCodebaseTool::new(config);

    let result = tool
        .execute(json!({
            "pattern": "search_target",
            "glob": "*.rs"
        }))
        .await
        .unwrap();

    let output = result.output.unwrap();
    let matches: Vec<Value> = serde_json::from_str(output.as_str().unwrap())
        .expect("output should be a JSON array string");

    assert_eq!(matches.len(), 1, "expected exactly 1 match (only the .rs file)");
    let file = matches[0]["file"].as_str().unwrap_or("");
    assert!(
        file.ends_with(".rs"),
        "expected matched file to end with '.rs', got: {}",
        file
    );
}
