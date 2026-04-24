//! Integration tests for amplifier-module-tool-bash.
//!
//! Tests cover safety profiles (Android allowlist, Strict denylist) and
//! command execution behavior.

use amplifier_module_tool_bash::{BashConfig, BashTool, SafetyProfile};
use amplifier_core::traits::Tool;
use serde_json::json;
use std::path::PathBuf;

/// Helper: construct a BashTool with the given profile, /tmp working dir, 5s timeout.
fn make_tool(profile: SafetyProfile) -> BashTool {
    BashTool::new(BashConfig {
        safety_profile: profile,
        working_dir: PathBuf::from("/tmp"),
        timeout_secs: 5,
    })
}

/// Android profile: 'echo hello' is in toybox allowlist → should succeed with output 'hello'.
#[tokio::test]
async fn android_profile_allows_toybox_commands() {
    let tool = make_tool(SafetyProfile::Android);
    let result = tool.execute(json!({"command": "echo hello"})).await.unwrap();
    let output = result.output.unwrap();
    assert_eq!(output.as_str().unwrap().trim(), "hello");
}

/// Android profile: 'sudo' is not in allowlist → Err with message containing 'toybox allowlist'.
#[tokio::test]
async fn android_profile_rejects_sudo() {
    let tool = make_tool(SafetyProfile::Android);
    let err = tool.execute(json!({"command": "sudo ls"})).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("toybox allowlist"),
        "Expected 'toybox allowlist' in error message, got: {}",
        msg
    );
}

/// Android profile: 'python3' is not in toybox allowlist → should fail.
#[tokio::test]
async fn android_profile_rejects_python() {
    let tool = make_tool(SafetyProfile::Android);
    let result = tool.execute(json!({"command": r#"python3 -c "print(1)""#})).await;
    assert!(result.is_err(), "Expected python3 to be rejected by Android profile");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("toybox allowlist"),
        "Expected 'toybox allowlist' in: {}",
        msg
    );
}

/// Android profile: 'ls /tmp' — 'ls' is in toybox allowlist, no profile error.
/// The command may fail for other reasons but must NOT fail with 'toybox allowlist' message.
#[tokio::test]
async fn android_profile_allows_ls_with_args() {
    let tool = make_tool(SafetyProfile::Android);
    let result = tool.execute(json!({"command": "ls /tmp"})).await;
    match result {
        Ok(_) => {} // success is fine
        Err(err) => {
            let msg = err.to_string();
            assert!(
                !msg.contains("toybox allowlist"),
                "Should not fail with toybox allowlist message for 'ls', but got: {}",
                msg
            );
        }
    }
}

/// Strict profile: 'rm -rf' pattern is in deny list → Err containing 'blocked' or 'rm -rf'.
#[tokio::test]
async fn strict_profile_blocks_rm_rf() {
    let tool = make_tool(SafetyProfile::Strict);
    let err = tool
        .execute(json!({"command": "rm -rf /tmp/test"}))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("blocked") || msg.contains("rm -rf"),
        "Expected 'blocked' or 'rm -rf' in: {}",
        msg
    );
}

/// Strict profile: 'sudo' is in deny list → command blocked.
#[tokio::test]
async fn strict_profile_blocks_sudo() {
    let tool = make_tool(SafetyProfile::Strict);
    let err = tool
        .execute(json!({"command": "sudo apt-get update"}))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        !msg.is_empty(),
        "Expected non-empty error for sudo under Strict profile"
    );
}

/// Strict profile: 'echo' has no deny-listed patterns → succeeds, output contains 'strict_ok'.
#[tokio::test]
async fn strict_profile_allows_echo() {
    let tool = make_tool(SafetyProfile::Strict);
    let result = tool
        .execute(json!({"command": "echo strict_ok"}))
        .await
        .unwrap();
    let output = result.output.unwrap();
    assert!(
        output.as_str().unwrap().contains("strict_ok"),
        "Expected 'strict_ok' in output, got: {}",
        output
    );
}
