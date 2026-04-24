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
