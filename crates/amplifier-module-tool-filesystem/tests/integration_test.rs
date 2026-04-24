use amplifier_module_tool_filesystem::FilesystemConfig;
use tempfile::TempDir;

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
