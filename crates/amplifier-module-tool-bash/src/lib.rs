//! Bash tool for the Amplifier agent framework.
//!
//! This crate provides [`BashTool`], which implements the
//! `amplifier_core::traits::Tool` interface for executing shell commands.
//!
//! # Safety Profiles
//!
//! Commands are gated by a [`SafetyProfile`] configured at construction time.
//! The **Android profile** is the Phase 3 addition, providing allowlist-based
//! safety for Android/toybox environments (see [`profiles::ANDROID_TOYBOX_ALLOWLIST`]).

pub mod profiles;

pub use profiles::SafetyProfile;

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

use crate::profiles::check_command;

// ---------------------------------------------------------------------------
// BashConfig
// ---------------------------------------------------------------------------

/// Configuration for [`BashTool`].
#[derive(Debug, Clone)]
pub struct BashConfig {
    /// Safety profile gating which commands are permitted.
    pub safety_profile: SafetyProfile,
    /// Working directory for command execution.
    pub working_dir: PathBuf,
    /// Maximum command timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for BashConfig {
    fn default() -> Self {
        Self {
            safety_profile: SafetyProfile::Strict,
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            timeout_secs: 30,
        }
    }
}

// ---------------------------------------------------------------------------
// BashTool
// ---------------------------------------------------------------------------

/// Tool for executing shell commands under a configurable safety profile.
///
/// # Example
///
/// ```rust,no_run
/// use amplifier_module_tool_bash::{BashConfig, BashTool, SafetyProfile};
///
/// let config = BashConfig {
///     safety_profile: SafetyProfile::Strict,
///     ..Default::default()
/// };
/// let tool = BashTool::new(config);
/// ```
pub struct BashTool {
    config: Arc<BashConfig>,
    description: String,
}

impl BashTool {
    /// Create a new [`BashTool`] from the given configuration.
    ///
    /// The config is wrapped in an `Arc` for cheap cloning across async tasks.
    pub fn new(config: BashConfig) -> Self {
        let description = format!(
            "Execute shell commands using /bin/sh. Safety profile: {:?}. Default timeout: {}s.",
            config.safety_profile, config.timeout_secs
        );
        Self {
            config: Arc::new(config),
            description,
        }
    }
}

// ---------------------------------------------------------------------------
// Async execution core
// ---------------------------------------------------------------------------

async fn execute_bash(config: Arc<BashConfig>, input: Value) -> Result<ToolResult, ToolError> {
    // Extract required `command` parameter.
    let command = input
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::ExecutionFailed {
            message: "missing required parameter: 'command'".to_string(),
            stdout: None,
            stderr: None,
            exit_code: None,
        })?;

    // Safety profile check.
    check_command(&config.safety_profile, command).map_err(|msg| ToolError::ExecutionFailed {
        message: msg,
        stdout: None,
        stderr: None,
        exit_code: None,
    })?;

    // Capped timeout: min(input_timeout, config_timeout).
    let timeout_secs = input
        .get("timeout")
        .and_then(|v| v.as_u64())
        .map(|t| t.min(config.timeout_secs))
        .unwrap_or(config.timeout_secs);

    let duration = std::time::Duration::from_secs(timeout_secs);

    // Execute command with timeout.
    let output_result = tokio::time::timeout(
        duration,
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&config.working_dir)
            .output(),
    )
    .await;

    match output_result {
        Err(_elapsed) => Err(ToolError::ExecutionFailed {
            message: format!("Command timed out after {}s", timeout_secs),
            stdout: None,
            stderr: None,
            exit_code: None,
        }),

        Ok(Err(io_err)) => Err(ToolError::ExecutionFailed {
            message: format!("Failed to execute command: {}", io_err),
            stdout: None,
            stderr: None,
            exit_code: None,
        }),

        Ok(Ok(output)) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                Ok(ToolResult {
                    success: true,
                    output: Some(json!(stdout)),
                    error: None,
                })
            } else {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code();
                Err(ToolError::ExecutionFailed {
                    message: stderr.clone(),
                    stdout: Some(stdout),
                    stderr: Some(stderr),
                    exit_code,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tool impl
// ---------------------------------------------------------------------------

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "command".to_string(),
            json!({
                "type": "string",
                "description": "Shell command to execute via /bin/sh -c"
            }),
        );

        properties.insert(
            "timeout".to_string(),
            json!({
                "type": "integer",
                "description": "Optional timeout in seconds (capped at configured maximum)"
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["command"]));

        ToolSpec {
            name: "bash".to_string(),
            parameters,
            description: Some(self.description.clone()),
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(execute_bash(self.config.clone(), input))
    }
}
