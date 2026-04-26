//! amplifier-module-tool-delegate — delegate tool for spawning named sub-agents.
//!
//! This crate provides the [`DelegateTool`] which enables agents to delegate
//! work to named sub-agents from an [`AgentRegistry`].

/// Conversation context extraction helpers.
pub mod context;
/// Agent registry resolver.
pub mod resolver;

use std::sync::Arc;

use amplifier_module_agent_runtime::AgentRegistry;
use amplifier_module_session_store::SessionStore;

pub use amplifier_module_tool_task::{ContextDepth, ContextScope, SpawnRequest, SubagentRunner};

// ---------------------------------------------------------------------------
// DelegateConfig
// ---------------------------------------------------------------------------

/// Configuration for the delegate tool.
#[derive(Debug, Clone)]
pub struct DelegateConfig {
    /// Maximum depth for self-delegation (recursive invocation). Default: 3.
    pub max_self_delegation_depth: usize,
    /// Maximum number of conversation turns to pass as context. Default: 10.
    pub max_context_turns: usize,
    /// Tool names to exclude from child sessions. Default: `["delegate"]`.
    pub exclude_tools: Vec<String>,
    /// Maximum wall-clock time for a child session. Default: `None` (disabled).
    ///
    /// Mirrors Python `settings.timeout` — disabled by default. Set only when
    /// you need a hard wall-clock cap; otherwise the sub-agent runs to completion.
    pub timeout: Option<std::time::Duration>,
}

impl Default for DelegateConfig {
    fn default() -> Self {
        Self {
            max_self_delegation_depth: 3,
            max_context_turns: 10,
            exclude_tools: vec!["delegate".to_string()],
            timeout: None, // disabled — matches Python `settings.timeout` default
        }
    }
}

// ---------------------------------------------------------------------------
// DelegateTool
// ---------------------------------------------------------------------------

/// Tool that enables an agent to delegate work to a named sub-agent.
pub struct DelegateTool {
    runner: Arc<dyn SubagentRunner>,
    registry: Arc<AgentRegistry>,
    config: DelegateConfig,
    /// Optional session store for resume path.
    store: Option<Arc<dyn SessionStore>>,
}

impl DelegateTool {
    /// Create a new [`DelegateTool`].
    pub fn new(
        runner: Arc<dyn SubagentRunner>,
        registry: Arc<AgentRegistry>,
        config: DelegateConfig,
    ) -> Self {
        Self {
            runner,
            registry,
            config,
            store: None,
        }
    }

    /// Create a [`DelegateTool`] backed by a session store (enables `session_id` resume).
    pub fn new_with_store(
        runner: Arc<dyn SubagentRunner>,
        registry: Arc<AgentRegistry>,
        config: DelegateConfig,
        store: Arc<dyn SessionStore>,
    ) -> Self {
        Self {
            runner,
            registry,
            config,
            store: Some(store),
        }
    }
}

// ---------------------------------------------------------------------------
// generate_sub_session_id
// ---------------------------------------------------------------------------

/// Generate a unique sub-session ID derived from a parent session ID and agent name.
///
/// Format: `"{parent_id}-{16hex}_{slug}"`
/// where `16hex` is a random `u64` formatted as 16 lowercase hex digits, and
/// `slug` is `agent_name` with `/`, `:`, and ` ` replaced by `-`.
///
/// # Examples
/// ```ignore
/// let id = generate_sub_session_id("session-abc", "foundation:explorer");
/// // → "session-abc-a1b2c3d4e5f60718_foundation-explorer"
/// ```
pub fn generate_sub_session_id(parent_id: &str, agent_name: &str) -> String {
    let hex = format!("{:016x}", rand::random::<u64>());
    let slug = agent_name.replace(['/', ':', ' '], "-");
    format!("{}-{}_{}", parent_id, hex, slug)
}

// ---------------------------------------------------------------------------
// Tool trait implementation
// ---------------------------------------------------------------------------

use amplifier_core::errors::ToolError;
use amplifier_core::messages::ToolSpec;
use amplifier_core::models::ToolResult;
use amplifier_core::traits::Tool;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        "Spawn a specialist sub-agent to handle a task autonomously. \
         Delegate aggressively — direct tool use only for trivial single-step work."
    }

    fn get_spec(&self) -> ToolSpec {
        // ── Build dynamic agent catalogue from the registry ──────────────────
        let agent_lines: Vec<String> = {
            let mut names = self.registry.available_names();
            names.sort();
            names
                .into_iter()
                .map(|name| {
                    let desc = self.registry
                        .get(name)
                        .map(|c| c.description.as_str())
                        .unwrap_or("specialist sub-agent");
                    format!("  - \"{name}\": {desc}")
                })
                .collect()
        };
        let agents_section = if agent_lines.is_empty() {
            "  (no agents registered — add .md files to <vault>/.agents/)".to_string()
        } else {
            agent_lines.join("\n")
        };

        // ── Rich description — mirrors Python amplifier-module-tool-delegate ──
        let description = format!(
            r#"Spawn a specialist sub-agent to handle a task autonomously.

## DEFAULT BEHAVIOUR: delegate first

Direct tool use (read_file, bash, grep…) should be RARE — only for trivial single-step lookups.
For anything beyond that, delegate to the right specialist and synthesise their result.

## Available agents

{agents_section}

Use "self" to spawn a fresh instance of yourself (token conservation).

## Choosing the right agent

| Task | Agent |
|------|-------|
| Explore files, survey a codebase, read multiple files | "explorer" |
| Architecture decisions, code review, design | "zen-architect" |
| Debug errors, find root cause of failures | "bug-hunter" |
| git commit, branch, PR, GitHub operations | "git-ops" |
| Implement a feature from a spec | "modular-builder" |
| Security review, vulnerability analysis | "security-guardian" |

## Writing a good instruction

Include WHAT and WHY — the sub-agent starts with no prior context.

BAD:  "look at the code"
GOOD: "Survey all Rust crates in /workspace/amplifier-rust/crates/ and return a table
       showing crate name, purpose (tool/provider/orchestrator/context), and key public types."

## Session resume

The result includes a session_id. Pass it back as session_id to continue the same agent
session — it remembers prior context and tool results from the previous call."#,
            agents_section = agents_section,
        );

        // ── Parameter schemas ─────────────────────────────────────────────────
        let mut properties = HashMap::new();

        properties.insert(
            "agent".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Name of the agent to spawn. Use a bare name from the list above \
                                (e.g. \"explorer\"), or \"self\" to spawn a fresh copy of yourself."
            }),
        );

        properties.insert(
            "instruction".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "What you want the sub-agent to do. Be specific — include the \
                                goal, relevant paths or identifiers, and what you want back. \
                                The sub-agent has NO prior context from this conversation."
            }),
        );

        properties.insert(
            "context_depth".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "How much of the current conversation to share with the sub-agent. \
                                \"none\" — fresh start, no prior context (fastest). \
                                \"recent\" — last 5 turns (default, usually enough). \
                                \"all\" — full conversation history (use when sub-agent needs \
                                         deep context, e.g. continuing a long investigation).",
                "enum": ["none", "recent", "all"]
            }),
        );

        properties.insert(
            "context_scope".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Which parts of the conversation to include. \
                                \"conversation\" — user/assistant text only (default, safest). \
                                \"agents\" — also include results from prior delegate calls \
                                            (chain agents so B sees A's output). \
                                \"full\" — all tool results too (complete mirror of this session).",
                "enum": ["conversation", "agents", "full"]
            }),
        );

        properties.insert(
            "session_id".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Resume a previous sub-agent session. Omit to start fresh. \
                                Pass the session_id from a prior delegate result to continue \
                                the same agent with its accumulated context and tool history."
            }),
        );

        ToolSpec {
            name: "delegate".to_string(),
            description: Some(description),
            parameters: {
                let mut params = HashMap::new();
                params.insert("type".to_string(), serde_json::json!("object"));
                params.insert("properties".to_string(), serde_json::json!(properties));
                params.insert(
                    "required".to_string(),
                    serde_json::json!(["agent", "instruction"]),
                );
                params
            },
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        let runner = Arc::clone(&self.runner);
        let registry = Arc::clone(&self.registry);
        let store = self.store.clone();
        let config_timeout = self.config.timeout; // None by default — matches Python spec
        let config_exclude_tools = self.config.exclude_tools.clone();
        let config_max_context_turns = self.config.max_context_turns;
        Box::pin(async move {
            let agent = input["agent"]
                .as_str()
                .ok_or_else(|| ToolError::Other {
                    message: "agent is required".into(),
                })?
                .to_string();
            let instruction = input["instruction"]
                .as_str()
                .ok_or_else(|| ToolError::Other {
                    message: "instruction is required".into(),
                })?
                .to_string();

            // Parse optional session_id for resume path.
            let session_id = input
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(String::from);

            // Parse context_depth (default: Recent(5), matching Python reference).
            let context_depth = match input.get("context_depth").and_then(|v| v.as_str()) {
                Some("none") => amplifier_module_tool_task::ContextDepth::None,
                Some("all") => amplifier_module_tool_task::ContextDepth::All,
                _ => amplifier_module_tool_task::ContextDepth::Recent(config_max_context_turns),
            };

            // Parse context_scope (default: Conversation).
            let context_scope = match input.get("context_scope").and_then(|v| v.as_str()) {
                Some("agents") => amplifier_module_tool_task::ContextScope::Agents,
                Some("full") => amplifier_module_tool_task::ContextScope::Full,
                _ => amplifier_module_tool_task::ContextScope::Conversation,
            };

            // Resolve agent system prompt from registry if available.
            let agent_system_prompt = registry.get(&agent).map(|c| c.instruction.clone());

            let (response_text, used_session_id, turn_count, tools_called): (String, String, usize, Vec<String>) = if let Some(sid) = session_id
            {
                // Resume path — requires an attached store.
                let store = store.as_ref().ok_or_else(|| ToolError::Other {
                    message: "session_id provided but no SessionStore configured".into(),
                })?;
                if !store.exists(&sid).await {
                    return Err(ToolError::Other {
                        message: format!("session not found: {sid}"),
                    });
                }
                let resume_fut = runner.resume(&sid, instruction);
                let spawn_result = if let Some(dur) = config_timeout {
                    tokio::time::timeout(dur, resume_fut)
                        .await
                        .map_err(|_| ToolError::Other {
                            message: format!(
                                "Agent '{}' timed out after {}s (delegate tool session-level \
                                 timeout). Increase or disable timeout via DelegateConfig.timeout.",
                                agent, dur.as_secs()
                            ),
                        })?
                        .map_err(|e| ToolError::ExecutionFailed {
                            message: e.to_string(),
                            stdout: None, stderr: None, exit_code: None,
                        })?
                } else {
                    resume_fut.await.map_err(|e| ToolError::ExecutionFailed {
                        message: e.to_string(),
                        stdout: None, stderr: None, exit_code: None,
                    })?
                };
                (spawn_result.response, spawn_result.session_id, spawn_result.turn_count, spawn_result.tools_called)
            } else {
                // Normal run path.
                let req = SpawnRequest {
                    instruction,
                    context_depth,
                    context_scope,
                    context: vec![],
                    session_id: None,
                    agent_system_prompt,
                    tool_filter: config_exclude_tools,
                };
                log::info!("[delegate] calling runner.run() for agent=\"{}\"", agent);
                let run_fut = runner.run(req);
                let spawn_result = if let Some(dur) = config_timeout {
                    tokio::time::timeout(dur, run_fut)
                        .await
                        .map_err(|_| ToolError::Other {
                            message: format!(
                                "Agent '{}' timed out after {}s (delegate tool session-level \
                                 timeout). Increase or disable timeout via DelegateConfig.timeout.",
                                agent, dur.as_secs()
                            ),
                        })?
                        .map_err(|e| ToolError::ExecutionFailed {
                            message: e.to_string(),
                            stdout: None, stderr: None, exit_code: None,
                        })?
                } else {
                    run_fut.await.map_err(|e| ToolError::ExecutionFailed {
                        message: e.to_string(),
                        stdout: None, stderr: None, exit_code: None,
                    })?
                };
                (spawn_result.response, spawn_result.session_id, spawn_result.turn_count, spawn_result.tools_called)
            };

            // Return rich JSON result matching Python reference format:
            // { response, agent, status, turn_count, session_id, tools_called }
            let json_result = serde_json::json!({
                "response": response_text,
                "agent": agent,
                "status": "success",
                "turn_count": turn_count,
                "session_id": used_session_id,
                "tools_called": tools_called,
            });

            Ok(ToolResult {
                success: true,
                output: Some(json_result),
                error: None,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use amplifier_module_agent_runtime::AgentRegistry;
    use std::sync::Arc;

    struct NopRunner;

    #[async_trait::async_trait]
    impl SubagentRunner for NopRunner {
        async fn run(&self, _req: SpawnRequest) -> anyhow::Result<amplifier_module_tool_task::SpawnResult> {
            Ok(amplifier_module_tool_task::SpawnResult {
                response: "nop".to_string(),
                session_id: String::new(),
                turn_count: 1,
                tools_called: vec![],
            })
        }
    }

    // --- Test 1: delegate_tool_can_be_constructed ---

    /// Verify that DelegateTool::new accepts runner, registry, and config.
    #[test]
    fn delegate_tool_can_be_constructed() {
        let runner: Arc<dyn SubagentRunner> = Arc::new(NopRunner);
        let registry = Arc::new(AgentRegistry::new());
        let config = DelegateConfig::default();
        let _tool = DelegateTool::new(runner, registry, config);
    }

    // --- Test 2: delegate_config_defaults ---

    /// Verify DelegateConfig::default() values match specification.
    #[test]
    fn delegate_config_defaults() {
        let config = DelegateConfig::default();
        assert_eq!(config.max_self_delegation_depth, 3);
        assert_eq!(config.max_context_turns, 10);
        assert_eq!(config.exclude_tools, vec!["delegate".to_string()]);
    }

    // --- Test 3: generate_sub_session_id_format ---

    /// Verify the generated session ID format:
    /// - Starts with `"{parent_id}-"`
    /// - Ends with `"_{slug}"`
    /// - Middle hex part is exactly 16 lowercase hex digits.
    #[test]
    fn generate_sub_session_id_format() {
        let id = generate_sub_session_id("parent", "explorer");
        assert!(
            id.starts_with("parent-"),
            "expected 'parent-' prefix, got: {id}"
        );
        assert!(
            id.ends_with("_explorer"),
            "expected '_explorer' suffix, got: {id}"
        );
        // Extract the hex segment between prefix and suffix
        let without_prefix = id.strip_prefix("parent-").unwrap();
        let without_suffix = without_prefix.strip_suffix("_explorer").unwrap();
        assert_eq!(
            without_suffix.len(),
            16,
            "expected 16-char hex, got: {}",
            without_suffix
        );
        assert!(
            without_suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "expected all hex digits, got: {without_suffix}"
        );
    }

    // --- Test 4: generate_sub_session_id_slugifies_special_chars ---

    /// Verify that '/', ':', and ' ' in the agent name are replaced with '-'.
    #[test]
    fn generate_sub_session_id_slugifies_special_chars() {
        let id = generate_sub_session_id("parent", "my/namespace:agent name");
        assert!(
            id.ends_with("_my-namespace-agent-name"),
            "expected '_my-namespace-agent-name' suffix, got: {id}"
        );
    }

    // --- Test 5: delegate_returns_error_when_session_id_unknown ---

    #[tokio::test]
    async fn delegate_returns_error_when_session_id_unknown() {
        use amplifier_module_session_store::FileSessionStore;

        let tmp = tempfile::TempDir::new().unwrap();
        let store = Arc::new(FileSessionStore::new_with_root(tmp.path().to_path_buf()));
        let runner: Arc<dyn SubagentRunner> = Arc::new(NopRunner);
        let registry = Arc::new(AgentRegistry::new());
        let tool = DelegateTool::new_with_store(
            runner,
            registry,
            DelegateConfig::default(),
            store,
        );

        let input = serde_json::json!({
            "agent": "explorer",
            "instruction": "do something",
            "session_id": "does-not-exist",
        });
        let res = tool.execute(input).await;
        let err = res.expect_err("missing session must error");
        assert!(
            err.to_string().contains("session not found") || err.to_string().contains("does-not-exist"),
            "expected session not found error, got: {err}"
        );
    }

    // --- Test 6: delegate_calls_resume_when_session_id_present ---

    #[tokio::test]
    async fn delegate_calls_resume_when_session_id_present() {
        use amplifier_module_session_store::{FileSessionStore, SessionMetadata, SessionStore};
        use amplifier_module_tool_task::SpawnResult;

        let tmp = tempfile::TempDir::new().unwrap();
        let store = Arc::new(FileSessionStore::new_with_root(tmp.path().to_path_buf()));

        // Pre-create a real session so exists() returns true
        store.begin("real-sid", SessionMetadata {
            session_id: "real-sid".into(),
            agent_name: "explorer".into(),
            parent_id: None,
            created: chrono::Utc::now().to_rfc3339(),
            status: "active".into(),
        }).await.unwrap();

        use std::sync::Mutex as StdMutex;
        struct ResumeRecorder { called: Arc<StdMutex<Option<String>>> }
        #[async_trait::async_trait]
        impl SubagentRunner for ResumeRecorder {
            async fn run(&self, _req: SpawnRequest) -> anyhow::Result<SpawnResult> {
                *self.called.lock().unwrap() = Some("run".into());
                Ok(SpawnResult { response: "from run".into(), session_id: String::new(), turn_count: 1, tools_called: vec![] })
            }
            async fn resume(&self, session_id: &str, _instruction: String) -> anyhow::Result<SpawnResult> {
                *self.called.lock().unwrap() = Some(format!("resume:{session_id}"));
                Ok(SpawnResult { response: "from resume".into(), session_id: session_id.into(), turn_count: 1, tools_called: vec![] })
            }
        }

        let called = Arc::new(StdMutex::new(None));
        let runner: Arc<dyn SubagentRunner> = Arc::new(ResumeRecorder { called: called.clone() });
        let registry = Arc::new(AgentRegistry::new());
        let tool = DelegateTool::new_with_store(
            runner, registry, DelegateConfig::default(), store,
        );

        let input = serde_json::json!({
            "agent": "explorer",
            "instruction": "continue",
            "session_id": "real-sid",
        });
        let res = tool.execute(input).await;
        assert!(res.is_ok(), "resume path should succeed: {:?}", res);
        assert_eq!(*called.lock().unwrap(), Some("resume:real-sid".into()));
    }
}
