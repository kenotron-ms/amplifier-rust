//! Task tool module — provides the `spawn_agent` tool for recursive agent delegation.
//!
//! This module defines [`TaskTool`], which implements the `amplifier_core::traits::Tool`
//! interface and enables agents to spawn sub-agents via the [`SubagentRunner`] trait.
//!
//! # Recursion guard
//!
//! [`TaskTool`] carries `current_depth` and `max_recursion_depth` fields. Before
//! executing, it checks `current_depth >= max_recursion_depth` and returns a
//! [`ToolError::ExecutionFailed`] if the limit is exceeded.
//!
//! # No dependency on orchestrator
//!
//! This crate is deliberately free of any dependency on the orchestrator crate
//! to avoid circular dependency cycles.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Value};

use amplifier_core::errors::ToolError;
use amplifier_core::messages::ToolSpec;
use amplifier_core::models::ToolResult;
use amplifier_core::traits::Tool;

// ---------------------------------------------------------------------------
// ContextDepth
// ---------------------------------------------------------------------------

/// How much conversation history to pass to the subagent.
#[derive(Debug, Clone, PartialEq)]
pub enum ContextDepth {
    /// No context — the sub-agent starts fresh.
    None,
    /// Last N turns of context.
    Recent(usize),
    /// Full conversation history.
    All,
}

// ---------------------------------------------------------------------------
// ContextScope
// ---------------------------------------------------------------------------

/// Which categories of content to include in the context.
#[derive(Debug, Clone, PartialEq)]
pub enum ContextScope {
    /// Human/assistant conversation text only.
    Conversation,
    /// Conversation + results from previous agent delegations.
    Agents,
    /// Everything, including raw tool call results.
    Full,
}

// ---------------------------------------------------------------------------
// SpawnRequest
// ---------------------------------------------------------------------------

/// A request to spawn a sub-agent.
#[derive(Debug)]
pub struct SpawnRequest {
    /// The instruction to pass to the sub-agent.
    pub instruction: String,
    /// How much context history to pass to the sub-agent.
    pub context_depth: ContextDepth,
    /// Which categories of content to include.
    pub context_scope: ContextScope,
    /// Additional context messages.
    pub context: Vec<Value>,
    /// Optional session ID to resume a previous sub-agent session.
    pub session_id: Option<String>,
    /// Agent system prompt override; None = use parent orchestrator's.
    pub agent_system_prompt: Option<String>,
    /// Tool name allowlist for child session; empty = inherit all parent tools.
    pub tool_filter: Vec<String>,
}

// ---------------------------------------------------------------------------
// SpawnResult
// ---------------------------------------------------------------------------

/// Result of a sub-agent run or resume.
///
/// Carries both the response text and the session ID so callers can persist
/// or resume later.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnResult {
    /// The text response from the sub-agent.
    pub response: String,
    /// The session ID used for this run (enables future resume calls).
    pub session_id: String,
    /// Number of LLM turns taken during the run. Defaults to 1 until the
    /// orchestrator's SubagentRunner::run() tracks steps explicitly.
    pub turn_count: usize,
}

// ---------------------------------------------------------------------------
// SubagentRunner
// ---------------------------------------------------------------------------

/// Trait for running sub-agent sessions.
///
/// Implementors drive a sub-agent to completion and return its text response.
#[async_trait::async_trait]
pub trait SubagentRunner: Send + Sync {
    /// Run a sub-agent with the given request.
    async fn run(&self, req: SpawnRequest) -> anyhow::Result<String>;

    /// Resume an existing session by `session_id`, appending `instruction`
    /// as the next user turn.
    ///
    /// The default implementation returns `Err("resume not supported")`.
    /// Implementors with a session store should override this.
    async fn resume(
        &self,
        _session_id: &str,
        _instruction: String,
    ) -> anyhow::Result<SpawnResult> {
        anyhow::bail!("resume not supported by this runner")
    }
}

// ---------------------------------------------------------------------------
// TaskTool
// ---------------------------------------------------------------------------

/// Tool that enables an agent to spawn sub-agents.
///
/// Implements [`amplifier_core::traits::Tool`] and provides the `spawn_agent`
/// tool name in its spec.
pub struct TaskTool {
    runner: Arc<dyn SubagentRunner>,
    max_recursion_depth: usize,
    current_depth: usize,
}

impl TaskTool {
    /// Create a new [`TaskTool`].
    ///
    /// # Arguments
    ///
    /// * `runner` — The [`SubagentRunner`] to use for spawning sub-agents.
    /// * `max_recursion_depth` — Maximum nesting depth allowed.
    /// * `current_depth` — Current nesting depth (starts at 0 for top-level).
    pub fn new(
        runner: Arc<dyn SubagentRunner>,
        max_recursion_depth: usize,
        current_depth: usize,
    ) -> Self {
        Self {
            runner,
            max_recursion_depth,
            current_depth,
        }
    }
}

impl Tool for TaskTool {
    fn name(&self) -> &str {
        "_spawn_agent"
    }

    fn description(&self) -> &str {
        "Spawn a specialized sub-agent to handle a task autonomously"
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "instruction".to_string(),
            json!({
                "type": "string",
                "description": "Clear instruction for the agent describing what to accomplish"
            }),
        );
        properties.insert(
            "context_depth".to_string(),
            json!({
                "type": "string",
                "enum": ["none", "recent_5", "all"],
                "description": "How much history to pass: none (clean slate), recent_5 (last 5 turns), all (full history)"
            }),
        );
        properties.insert(
            "context_scope".to_string(),
            json!({
                "type": "string",
                "enum": ["conversation", "agents", "full"],
                "description": "Which content categories: conversation (text only), agents (+ prior agent results), full (+ all tool results)"
            }),
        );
        properties.insert(
            "session_id".to_string(),
            json!({
                "type": "string",
                "description": "Optional session ID to resume a previous sub-agent session"
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["instruction"]));

        ToolSpec {
            name: "_spawn_agent".to_string(),
            parameters,
            description: Some(
                "Spawn a specialized sub-agent to handle a task autonomously".to_string(),
            ),
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(async move {
            // --- Recursion guard ---
            if self.current_depth >= self.max_recursion_depth {
                return Err(ToolError::ExecutionFailed {
                    message: format!(
                        "Recursion depth limit {} exceeded (current depth: {}). \
                         This call is non-recoverable.",
                        self.max_recursion_depth, self.current_depth
                    ),
                    stdout: None,
                    stderr: None,
                    exit_code: None,
                });
            }

            // --- Parse instruction (required) ---
            let instruction = match input.get("instruction").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => {
                    return Err(ToolError::Other {
                        message: "missing required parameter: 'instruction'".to_string(),
                    });
                }
            };

            // --- Parse context_depth (default: None) ---
            let context_depth = match input.get("context_depth").and_then(|v| v.as_str()) {
                Some("recent_5") => ContextDepth::Recent(5),
                Some("all") => ContextDepth::All,
                // "none" or absent => ContextDepth::None
                _ => ContextDepth::None,
            };

            // --- Parse context_scope (default: Conversation) ---
            let context_scope = match input.get("context_scope").and_then(|v| v.as_str()) {
                Some("agents") => ContextScope::Agents,
                Some("full") => ContextScope::Full,
                // "conversation" or absent => ContextScope::Conversation
                _ => ContextScope::Conversation,
            };

            // --- Parse session_id (optional) ---
            let session_id = input
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // --- Build request and call runner ---
            let req = SpawnRequest {
                instruction,
                context_depth,
                context_scope,
                context: vec![],
                session_id,
                agent_system_prompt: None,
                tool_filter: vec![],
            };

            match self.runner.run(req).await {
                Ok(result) => Ok(ToolResult {
                    success: true,
                    output: Some(Value::String(result)),
                    error: None,
                }),
                Err(e) => Err(ToolError::ExecutionFailed {
                    message: e.to_string(),
                    stdout: None,
                    stderr: None,
                    exit_code: None,
                }),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- Mock runners ---

    struct SuccessRunner {
        response: String,
    }

    #[async_trait::async_trait]
    impl SubagentRunner for SuccessRunner {
        async fn run(&self, _req: SpawnRequest) -> anyhow::Result<String> {
            Ok(self.response.clone())
        }
    }

    struct FailRunner;

    #[async_trait::async_trait]
    impl SubagentRunner for FailRunner {
        async fn run(&self, _req: SpawnRequest) -> anyhow::Result<String> {
            Err(anyhow::anyhow!("runner failed"))
        }
    }

    /// Captures the last `SpawnRequest` passed to `run()`.
    struct CapturingRunner {
        captured: Arc<std::sync::Mutex<Option<SpawnRequest>>>,
    }

    #[async_trait::async_trait]
    impl SubagentRunner for CapturingRunner {
        async fn run(&self, req: SpawnRequest) -> anyhow::Result<String> {
            *self.captured.lock().unwrap() = Some(req);
            Ok("captured".to_string())
        }
    }

    // --- Test 1: spawn_request_default_depth_is_none ---

    /// Verify that SpawnRequest can be constructed with ContextDepth::None
    /// as the depth value (the "natural default").
    #[test]
    fn spawn_request_default_depth_is_none() {
        let req = SpawnRequest {
            instruction: "do something".to_string(),
            context_depth: ContextDepth::None,
            context_scope: ContextScope::Conversation,
            context: vec![],
            session_id: None,
            agent_system_prompt: None,
            tool_filter: vec![],
        };
        assert_eq!(req.context_depth, ContextDepth::None);
    }

    // --- Test 2: get_spec_name_is_spawn_agent ---

    /// The tool spec must advertise the name '_spawn_agent' (underscore prefix = hidden from LLMs).
    #[test]
    fn get_spec_name_is_spawn_agent() {
        let runner = Arc::new(SuccessRunner {
            response: "ok".to_string(),
        });
        let tool = TaskTool::new(runner, 5, 0);
        let spec = tool.get_spec();
        assert_eq!(spec.name, "_spawn_agent");
    }

    // --- Test 3: execute_returns_runner_output_on_success ---

    /// A successful runner call should be surfaced as a ToolResult with
    /// success=true and output=Some(Value::String(response)).
    #[tokio::test]
    async fn execute_returns_runner_output_on_success() {
        let runner = Arc::new(SuccessRunner {
            response: "hello from subagent".to_string(),
        });
        let tool = TaskTool::new(runner, 5, 0);
        let input = json!({"instruction": "do something"});
        let result = tool.execute(input).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output, Some(json!("hello from subagent")));
        assert!(result.error.is_none());
    }

    // --- Test 4: execute_missing_instruction_returns_invalid_input_error ---

    /// Missing 'instruction' must return ToolError::Other (InvalidInput),
    /// NOT ToolError::ExecutionFailed (which is reserved for the recursion
    /// guard and runner failures).
    #[tokio::test]
    async fn execute_missing_instruction_returns_invalid_input_error() {
        let runner = Arc::new(SuccessRunner {
            response: "ok".to_string(),
        });
        let tool = TaskTool::new(runner, 5, 0);
        let input = json!({});
        let result = tool.execute(input).await;
        assert!(
            matches!(result, Err(ToolError::Other { .. })),
            "expected ToolError::Other for missing instruction, got: {:?}",
            result
        );
    }

    // --- Test 5: execute_respects_recursion_limit ---

    /// When current_depth >= max_recursion_depth, execute must return
    /// ToolError::ExecutionFailed (non-recoverable).
    #[tokio::test]
    async fn execute_respects_recursion_limit() {
        let runner = Arc::new(SuccessRunner {
            response: "ok".to_string(),
        });
        // current_depth == max_recursion_depth → limit exceeded
        let tool = TaskTool::new(runner, 3, 3);
        let input = json!({"instruction": "do something"});
        let result = tool.execute(input).await;
        assert!(
            matches!(result, Err(ToolError::ExecutionFailed { .. })),
            "expected ToolError::ExecutionFailed for recursion limit, got: {:?}",
            result
        );
    }

    // --- Test 6: execute_propagates_runner_failure ---

    /// When the runner returns an error, execute must propagate it as
    /// ToolError::ExecutionFailed.
    #[tokio::test]
    async fn execute_propagates_runner_failure() {
        let runner = Arc::new(FailRunner);
        let tool = TaskTool::new(runner, 5, 0);
        let input = json!({"instruction": "do something"});
        let result = tool.execute(input).await;
        assert!(
            matches!(result, Err(ToolError::ExecutionFailed { .. })),
            "expected ToolError::ExecutionFailed when runner fails, got: {:?}",
            result
        );
    }

    // --- Test 7: execute_parses_context_depth_recent ---

    /// context_depth="recent_5" must be parsed as ContextDepth::Recent(5).
    #[tokio::test]
    async fn execute_parses_context_depth_recent() {
        let captured = Arc::new(std::sync::Mutex::new(None::<SpawnRequest>));
        let runner = Arc::new(CapturingRunner {
            captured: captured.clone(),
        });
        let tool = TaskTool::new(runner, 5, 0);
        let input = json!({
            "instruction": "test",
            "context_depth": "recent_5"
        });
        let _ = tool.execute(input).await.unwrap();
        let guard = captured.lock().unwrap();
        let req = guard.as_ref().expect("runner was not called");
        assert_eq!(
            req.context_depth,
            ContextDepth::Recent(5),
            "expected ContextDepth::Recent(5) for 'recent_5' input"
        );
    }

    // --- Test 8: subagent_runner_resume_default_returns_unsupported ---

    #[tokio::test]
    async fn subagent_runner_resume_default_returns_unsupported() {
        // SuccessRunner should have the default resume() impl which returns an error.
        let runner = SuccessRunner { response: "ignored".into() };
        let res = runner.resume("sid", "do more".to_string()).await;
        let err = res.expect_err("default resume must return Err");
        assert!(
            err.to_string().to_lowercase().contains("resume not supported"),
            "expected 'resume not supported', got: {err}"
        );
    }

    // --- Test 9: spawn_result_constructible ---

    #[test]
    fn spawn_result_constructible() {
        let r = SpawnResult {
            response: "hi".into(),
            session_id: "s".into(),
            turn_count: 1,
        };
        assert_eq!(r.response, "hi");
        assert_eq!(r.session_id, "s");
        assert_eq!(r.turn_count, 1);
    }
}
