use amplifier_module_tool_filesystem::FilesystemConfig;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// WriteFileTool tests
// ---------------------------------------------------------------------------

/// Test that write_file creates the file (and parent dirs) and returns byte count.
#[tokio::test]
async fn write_file_creates_file_and_parent_dirs() {
    use amplifier_core::traits::Tool;
    use amplifier_module_tool_filesystem::WriteFileTool;
    use serde_json::json;

    let dir = TempDir::new().unwrap();
    let config = FilesystemConfig::new(dir.path().to_path_buf());
    let tool = WriteFileTool::new(config);

    let result = tool
        .execute(json!({
            "path": "subdir/nested/new.txt",
            "content": "hello world"
        }))
        .await
        .unwrap();

    // Output should mention the byte count (11 bytes for "hello world").
    let output = result.output.unwrap();
    let s = output.as_str().unwrap();
    assert!(s.contains("11"), "expected byte count '11' in output: {}", s);

    // File should exist on disk with correct content.
    let file_path = dir.path().join("subdir/nested/new.txt");
    assert!(file_path.exists(), "file should exist on disk");
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "hello world");
}

/// Test that write_file blocks path traversal outside the vault.
#[tokio::test]
async fn write_file_denied_outside_vault() {
    use amplifier_core::traits::Tool;
    use amplifier_module_tool_filesystem::WriteFileTool;
    use serde_json::json;

    let dir = TempDir::new().unwrap();
    let config = FilesystemConfig::new(dir.path().to_path_buf());
    let tool = WriteFileTool::new(config);

    let result = tool
        .execute(json!({
            "path": "../outside.txt",
            "content": "should not be written"
        }))
        .await;

    assert!(result.is_err(), "expected Err for path traversal outside vault");
}

// ---------------------------------------------------------------------------
// EditFileTool tests
// ---------------------------------------------------------------------------

/// Test that edit_file replaces only the first occurrence by default.
#[tokio::test]
async fn edit_file_replaces_single_occurrence() {
    use amplifier_core::traits::Tool;
    use amplifier_module_tool_filesystem::EditFileTool;
    use serde_json::json;
    use std::fs;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("code.rs");
    fs::write(&file_path, "fn foo() {}\nfn bar() {}\n").unwrap();

    let config = FilesystemConfig::new(dir.path().to_path_buf());
    let tool = EditFileTool::new(config);

    let result = tool
        .execute(json!({
            "path": "code.rs",
            "old_string": "fn foo()",
            "new_string": "fn qux()"
        }))
        .await
        .unwrap();

    // Output should mention 1 replacement.
    let output = result.output.unwrap();
    let s = output.as_str().unwrap();
    assert!(s.contains("1"), "expected '1' in output: {}", s);

    // File should have correct content.
    let content = fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("fn qux()"), "expected 'fn qux()' in file");
    assert!(content.contains("fn bar()"), "expected 'fn bar()' in file");
    assert!(!content.contains("fn foo()"), "expected 'fn foo()' to be gone");
}

/// Test that edit_file replaces all occurrences when replace_all=true.
#[tokio::test]
async fn edit_file_replace_all_replaces_all_occurrences() {
    use amplifier_core::traits::Tool;
    use amplifier_module_tool_filesystem::EditFileTool;
    use serde_json::json;
    use std::fs;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("words.txt");
    fs::write(&file_path, "foo foo foo\n").unwrap();

    let config = FilesystemConfig::new(dir.path().to_path_buf());
    let tool = EditFileTool::new(config);

    let result = tool
        .execute(json!({
            "path": "words.txt",
            "old_string": "foo",
            "new_string": "bar",
            "replace_all": true
        }))
        .await
        .unwrap();

    assert!(result.success);

    let content = fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "bar bar bar\n");
}

/// Test that edit_file returns an error when old_string is not found.
#[tokio::test]
async fn edit_file_errors_when_old_string_not_found() {
    use amplifier_core::traits::Tool;
    use amplifier_module_tool_filesystem::EditFileTool;
    use serde_json::json;
    use std::fs;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("file.txt");
    fs::write(&file_path, "hello world\n").unwrap();

    let config = FilesystemConfig::new(dir.path().to_path_buf());
    let tool = EditFileTool::new(config);

    let result = tool
        .execute(json!({
            "path": "file.txt",
            "old_string": "nonexistent_string",
            "new_string": "replacement"
        }))
        .await;

    assert!(result.is_err(), "expected Err when old_string not found");
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not found") || msg.contains("old_string"),
        "error message should mention 'not found' or 'old_string', got: {}",
        msg
    );
}

#[test]
fn config_constructs() {
    let dir = TempDir::new().unwrap();
    let config = FilesystemConfig::new(dir.path().to_path_buf());
    assert_eq!(config.vault_root, dir.path());
    assert_eq!(config.allowed_write_paths.len(), 1);
    assert!(config.allowed_read_paths.is_none());
}

// ---------------------------------------------------------------------------
// ReadFileTool tests
// ---------------------------------------------------------------------------

/// Test that read_file returns line-numbered output.
#[tokio::test]
async fn read_file_returns_numbered_lines() {
    use amplifier_core::traits::Tool;
    use amplifier_module_tool_filesystem::ReadFileTool;
    use serde_json::json;
    use std::fs;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("hello.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let config = FilesystemConfig::new(dir.path().to_path_buf());
    let tool = ReadFileTool::new(config);

    let result = tool.execute(json!({ "path": "hello.txt" })).await.unwrap();
    assert!(result.success);

    let output = result.output.unwrap();
    let s = output.as_str().unwrap();
    assert!(s.contains("   1\talpha"), "expected '   1\\talpha' in: {}", s);
    assert!(s.contains("   2\tbeta"), "expected '   2\\tbeta' in: {}", s);
    assert!(s.contains("   3\tgamma"), "expected '   3\\tgamma' in: {}", s);
}

/// Test that read_file respects offset and limit parameters.
#[tokio::test]
async fn read_file_respects_offset_and_limit() {
    use amplifier_core::traits::Tool;
    use amplifier_module_tool_filesystem::ReadFileTool;
    use serde_json::json;
    use std::fs;

    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("lines.txt"),
        "one\ntwo\nthree\nfour\nfive\n",
    )
    .unwrap();

    let config = FilesystemConfig::new(dir.path().to_path_buf());
    let tool = ReadFileTool::new(config);

    let result = tool
        .execute(json!({
            "path": "lines.txt",
            "offset": 1,
            "limit": 2
        }))
        .await
        .unwrap();

    let output = result.output.unwrap();
    let s = output.as_str().unwrap();
    assert!(s.contains("   2\ttwo"), "expected '   2\\ttwo' in: {}", s);
    assert!(
        s.contains("   3\tthree"),
        "expected '   3\\tthree' in: {}",
        s
    );
    assert!(!s.contains("one"), "unexpected 'one' in: {}", s);
    assert!(!s.contains("four"), "unexpected 'four' in: {}", s);
}

/// Test that read_file returns an error for a missing file.
#[tokio::test]
async fn read_file_error_on_missing_file() {
    use amplifier_core::traits::Tool;
    use amplifier_module_tool_filesystem::ReadFileTool;
    use serde_json::json;

    let dir = TempDir::new().unwrap();
    let config = FilesystemConfig::new(dir.path().to_path_buf());
    let tool = ReadFileTool::new(config);

    let result = tool
        .execute(json!({ "path": "nonexistent.txt" }))
        .await;
    assert!(result.is_err(), "expected Err for missing file");
}
