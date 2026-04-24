//! Tool registry for the amplifier-android-sandbox.
//!
//! [`TaskTool`] and [`SkillsTool`] are wired in main.rs after orchestrator
//! creation, since they require a reference to the orchestrator itself.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use amplifier_core::traits::Tool;
use amplifier_module_tool_bash::{BashConfig, BashTool, SafetyProfile};
use amplifier_module_tool_filesystem::{
    EditFileTool, FilesystemConfig, GlobTool, GrepTool, ReadFileTool, WriteFileTool,
};
use amplifier_module_tool_search::{GrepCodebaseTool, SearchConfig};
use amplifier_module_tool_todo::TodoTool;
use amplifier_module_tool_web::fetch::FetchUrlTool;

// ---------------------------------------------------------------------------
// ToolMap type alias
// ---------------------------------------------------------------------------

/// A named map of boxed tools used by the sandbox agent runner.
pub type ToolMap = HashMap<String, Box<dyn Tool + Send + Sync>>;

// ---------------------------------------------------------------------------
// build_registry
// ---------------------------------------------------------------------------

/// Build the sandbox tool registry, scoped to `vault`.
///
/// Returns a [`ToolMap`] containing the 9 core tools wired for the Android
/// sandbox environment. `TaskTool` and `SkillsTool` are intentionally excluded
/// here; they are added in `main.rs` after the orchestrator is constructed
/// because they need a handle to it.
pub fn build_registry(vault: &Path) -> anyhow::Result<ToolMap> {
    let vault_buf = vault.to_path_buf();

    let fs_config = FilesystemConfig::new(vault_buf.clone());
    let bash_config = BashConfig {
        safety_profile: SafetyProfile::Android,
        working_dir: vault_buf.clone(),
        timeout_secs: 30,
    };
    let search_config = SearchConfig::new(vault_buf.clone());

    let mut tools: ToolMap = HashMap::new();

    tools.insert(
        "read_file".to_string(),
        Box::new(ReadFileTool::new(Arc::clone(&fs_config))),
    );
    tools.insert(
        "write_file".to_string(),
        Box::new(WriteFileTool::new(Arc::clone(&fs_config))),
    );
    tools.insert(
        "edit_file".to_string(),
        Box::new(EditFileTool::new(Arc::clone(&fs_config))),
    );
    tools.insert(
        "glob".to_string(),
        Box::new(GlobTool::new(Arc::clone(&fs_config))),
    );
    tools.insert(
        "grep".to_string(),
        Box::new(GrepTool::new(Arc::clone(&fs_config))),
    );
    tools.insert("bash".to_string(), Box::new(BashTool::new(bash_config)));
    tools.insert("web_fetch".to_string(), Box::new(FetchUrlTool::new()));
    tools.insert(
        "grep_codebase".to_string(),
        Box::new(GrepCodebaseTool::new(search_config)),
    );
    tools.insert("todo".to_string(), Box::new(TodoTool::default()));

    Ok(tools)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `build_registry` returns a map with exactly 9 tools.
    ///
    /// Diagnostic message lists all keys present so failures are self-explaining.
    #[test]
    fn registry_has_nine_tools() {
        let dir = tempfile::TempDir::new().unwrap();
        let tools = build_registry(dir.path()).expect("build_registry should not fail");
        let mut keys: Vec<&str> = tools.keys().map(|s| s.as_str()).collect();
        keys.sort();
        assert_eq!(
            tools.len(),
            9,
            "expected 9 tools, found {}. Keys: {:?}",
            tools.len(),
            keys
        );
    }

    /// Verify that all nine expected tool names are present as keys in the map.
    #[test]
    fn all_expected_tool_names_present() {
        let dir = tempfile::TempDir::new().unwrap();
        let tools = build_registry(dir.path()).expect("build_registry should not fail");
        for name in &[
            "read_file",
            "write_file",
            "edit_file",
            "glob",
            "grep",
            "bash",
            "web_fetch",
            "grep_codebase",
            "todo",
        ] {
            assert!(tools.contains_key(*name), "missing tool: {}", name);
        }
    }
}
