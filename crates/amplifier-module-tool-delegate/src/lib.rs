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
}

impl Default for DelegateConfig {
    fn default() -> Self {
        Self {
            max_self_delegation_depth: 3,
            max_context_turns: 10,
            exclude_tools: vec!["delegate".to_string()],
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
    #[allow(dead_code)]
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
        "Spawn a named sub-agent to handle a task. Supports self-delegation for recursion,          namespace:path resolution, and agent registry lookup."
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();
        properties.insert(
            "agent".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Agent name to delegate to. Use 'self' for recursion,                                'namespace:path' for bundle agents, or a bare name for registry lookup."
            }),
        );
        properties.insert(
            "instruction".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "The instruction to give the sub-agent."
            }),
        );
        properties.insert(
            "context_depth".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "How much context to pass: none | recent | all",
                "enum": ["none", "recent", "all"]
            }),
        );

        ToolSpec {
            name: "delegate".to_string(),
            description: Some("Spawn a named sub-agent to handle a task.".to_string()),
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

            // Resolve agent system prompt from registry if available.
            let agent_system_prompt = registry.get(&agent).map(|c| c.instruction.clone());

            let response_text: String = if let Some(sid) = session_id {
                // Resume path — requires an attached store.
                let store = store.as_ref().ok_or_else(|| ToolError::Other {
                    message: "session_id provided but no SessionStore configured".into(),
                })?;
                if !store.exists(&sid).await {
                    return Err(ToolError::Other {
                        message: format!("session not found: {sid}"),
                    });
                }
                runner
                    .resume(&sid, instruction)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        message: e.to_string(),
                        stdout: None,
                        stderr: None,
                        exit_code: None,
                    })?
                    .response
            } else {
                // Normal run path.
                let req = SpawnRequest {
                    instruction,
                    context_depth: amplifier_module_tool_task::ContextDepth::None,
                    context_scope: amplifier_module_tool_task::ContextScope::Conversation,
                    context: vec![],
                    session_id: None,
                    agent_system_prompt,
                    tool_filter: vec![],
                };
                runner
                    .run(req)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        message: e.to_string(),
                        stdout: None,
                        stderr: None,
                        exit_code: None,
                    })?
            };

            Ok(ToolResult {
                success: true,
                output: Some(serde_json::Value::String(response_text)),
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
        async fn run(&self, _req: SpawnRequest) -> anyhow::Result<String> {
            Ok("nop".to_string())
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
            async fn run(&self, _req: SpawnRequest) -> anyhow::Result<String> {
                *self.called.lock().unwrap() = Some("run".into());
                Ok("from run".into())
            }
            async fn resume(&self, session_id: &str, _instruction: String) -> anyhow::Result<SpawnResult> {
                *self.called.lock().unwrap() = Some(format!("resume:{session_id}"));
                Ok(SpawnResult { response: "from resume".into(), session_id: session_id.into() })
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
