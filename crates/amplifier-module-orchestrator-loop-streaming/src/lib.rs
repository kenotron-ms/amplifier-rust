//! LoopOrchestrator — streaming agent loop with hooks, step limit, and tool dispatch.

/// Hook integration points for the agent loop (step start, end, tool call, etc.).
pub mod hooks;

pub use hooks::*;

use std::collections::HashMap;
use std::sync::Arc;

use amplifier_core::messages::{
    ChatRequest, ContentBlock, Message, MessageContent, Role, ToolSpec,
};
use amplifier_core::traits::{ContextManager, Provider, Tool};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use amplifier_module_context_simple::SimpleContext;
use amplifier_module_session_store::{SessionEvent, SessionStore};
use amplifier_module_tool_task::{SpawnRequest, SubagentRunner};

// ---------------------------------------------------------------------------
// ToolTrackerHook
// ---------------------------------------------------------------------------

/// Lightweight hook that accumulates names of every tool the sub-agent calls.
struct ToolTrackerHook {
    names: std::sync::Mutex<Vec<String>>,
}

impl ToolTrackerHook {
    fn new() -> Self {
        Self { names: std::sync::Mutex::new(vec![]) }
    }
    fn get(&self) -> Vec<String> {
        self.names.lock().unwrap().clone()
    }
}

// Implement Hook for Arc<ToolTrackerHook> so the Arc can be both registered
// with the HookRegistry (which owns it via Box<dyn Hook>) and retained by the
// caller for reading accumulated names after execution.
#[async_trait::async_trait]
impl Hook for std::sync::Arc<ToolTrackerHook> {
    fn events(&self) -> &[HookEvent] {
        &[HookEvent::ToolPre]
    }
    async fn handle(&self, ctx: &HookContext) -> HookResult {
        if let Some(name) = ctx.data.get("name").and_then(|v| v.as_str()) {
            self.names.lock().unwrap().push(name.to_string());
        }
        HookResult::Continue
    }
}

// ---------------------------------------------------------------------------
// LoopConfig
// ---------------------------------------------------------------------------

/// Configuration for the agent loop.
#[derive(Default)]
pub struct LoopConfig {
    /// Maximum number of provider roundtrips before aborting.
    /// `None` = unlimited (Python-default behaviour); `Some(n)` = safety cap.
    pub max_steps: Option<usize>,
    /// Optional system prompt prepended to every request.
    pub system_prompt: String,
}

// ---------------------------------------------------------------------------
// LoopOrchestrator
// ---------------------------------------------------------------------------

/// Agent-loop orchestrator with hook integration, step limit, and tool dispatch.
pub struct LoopOrchestrator {
    /// Loop configuration (max steps, system prompt).
    pub config: LoopConfig,
    /// Registered providers keyed by name.
    pub providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
    /// Registered tools keyed by name.
    pub tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    // Session persistence (optional)
    session_store: RwLock<Option<Arc<dyn SessionStore>>>,
    session_id: RwLock<Option<String>>,
    agent_name: RwLock<Option<String>>,
    parent_id: RwLock<Option<String>>,
}

impl LoopOrchestrator {
    /// Create a new orchestrator with the given configuration.
    pub fn new(config: LoopConfig) -> Self {
        Self {
            config,
            providers: RwLock::new(HashMap::new()),
            tools: RwLock::new(HashMap::new()),
            session_store: RwLock::new(None),
            session_id: RwLock::new(None),
            agent_name: RwLock::new(None),
            parent_id: RwLock::new(None),
        }
    }

    /// Attach a session store. After this call, `execute()` will persist events.
    pub fn attach_store(
        &self,
        store: Arc<dyn SessionStore>,
        session_id: String,
        agent_name: String,
        parent_id: Option<String>,
    ) {
        *self
            .session_store
            .try_write()
            .expect("attach_store contention") = Some(store);
        *self
            .session_id
            .try_write()
            .expect("attach_store contention") = Some(session_id);
        *self
            .agent_name
            .try_write()
            .expect("attach_store contention") = Some(agent_name);
        *self
            .parent_id
            .try_write()
            .expect("attach_store contention") = parent_id;
    }

    /// Persist a single event to the attached store. Errors are silently ignored
    /// so a storage failure doesn't abort an agent turn.
    async fn persist(&self, event: SessionEvent) {
        let store_opt = { self.session_store.read().await.clone() };
        let sid_opt = { self.session_id.read().await.clone() };
        if let (Some(store), Some(sid)) = (store_opt, sid_opt) {
            let _ = store.append(&sid, event).await;
        }
    }

    /// Finalize the session with the given status.
    pub async fn finish_store(&self, status: &str) -> anyhow::Result<()> {
        let store_opt = { self.session_store.read().await.clone() };
        let sid_opt = { self.session_id.read().await.clone() };
        if let (Some(store), Some(sid)) = (store_opt, sid_opt) {
            store.finish(&sid, status, 0).await?;
        }
        Ok(())
    }

    /// Load a prior session and run one more turn with `instruction`.
    pub async fn resume(
        &self,
        session_id: &str,
        instruction: String,
    ) -> anyhow::Result<amplifier_module_tool_task::SpawnResult> {
        let store = {
            self.session_store
                .read()
                .await
                .clone()
                .ok_or_else(|| anyhow::anyhow!("resume requires an attached SessionStore"))?
        };

        if !store.exists(session_id).await {
            anyhow::bail!("session not found: {session_id}");
        }

        let prior = store.load(session_id).await?;
        *self.session_id.write().await = Some(session_id.to_string());

        // Replay prior Turn events as conversation history.
        let history: Vec<serde_json::Value> = prior
            .iter()
            .filter_map(|e| {
                if let SessionEvent::Turn { role, content, .. } = e {
                    Some(serde_json::json!({"role": role, "content": content}))
                } else {
                    None
                }
            })
            .collect();

        let mut ctx = SimpleContext::new(history);
        let hooks = HookRegistry::new();
        let response = self
            .execute(instruction, &mut ctx, &hooks, |_| {})
            .await?;

        Ok(amplifier_module_tool_task::SpawnResult {
            turn_count: 1,
            response,
            session_id: session_id.to_string(),
            tools_called: vec![],
        })
    }

    /// Register a provider by name.
    pub async fn register_provider(&self, name: String, provider: Arc<dyn Provider>) {
        self.providers.write().await.insert(name, provider);
    }

    /// Register a tool (name is taken from tool.get_spec().name).
    pub async fn register_tool(&self, tool: Arc<dyn Tool>) {
        let name = tool.get_spec().name.clone();
        self.tools.write().await.insert(name, tool);
    }

    /// Return a snapshot clone of the providers map.
    pub async fn snapshot_providers(&self) -> HashMap<String, Arc<dyn Provider>> {
        self.providers.read().await.clone()
    }

    /// Return a snapshot clone of the tools map.
    pub async fn snapshot_tools(&self) -> HashMap<String, Arc<dyn Tool>> {
        self.tools.read().await.clone()
    }

    /// Run the agent loop for a single prompt.
    ///
    /// # Parameters
    /// * `prompt` — The user's input.
    /// * `context` — Mutable reference to a context manager.
    /// * `hooks` — Hook registry for lifecycle events.
    /// * `on_token` — Callback invoked with text segments as they become available.
    ///
    /// # Returns
    /// The final response text, or an error if the loop fails.
    /// Returns `Err("max_steps exceeded")` only when `max_steps` is `Some(n)`.
    pub async fn execute(
        &self,
        prompt: String,
        context: &mut dyn ContextManager,
        hooks: &HookRegistry,
        on_token: impl Fn(&str) + Send,
    ) -> anyhow::Result<String> {
        // 1. Emit SessionStart
        hooks
            .emit(HookEvent::SessionStart, json!({"prompt": prompt}))
            .await;

        // 2. Pick provider (prefer 'anthropic', else first)
        let providers = self.snapshot_providers().await;
        let provider = providers
            .get("anthropic")
            .or_else(|| providers.values().next())
            .ok_or_else(|| anyhow::anyhow!("no provider registered"))?
            .clone();

        // 3. Snapshot tools
        let tools = self.snapshot_tools().await;

        // 4. Add user message to context
        context
            .add_message(json!({"role": "user", "content": prompt}))
            .await
            .map_err(|e| anyhow::anyhow!("add_message failed: {e}"))?;

        // 4a. Persist user turn
        self.persist(SessionEvent::Turn {
            role: "user".into(),
            content: prompt.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
        .await;

        // 5. Build tool specs
        let tool_specs: Vec<ToolSpec> = tools.values().map(|t| t.get_spec()).collect();

        // 6. Agent loop
        let mut step: usize = 0;
        loop {
            // Step-limit guard (None = unlimited, matching Python orchestrator behaviour)
            if let Some(limit) = self.config.max_steps {
                if step >= limit {
                    return Err(anyhow::anyhow!("max_steps exceeded"));
                }
            }

            // a. Emit ProviderRequest hook; collect ephemeral injections and system addenda
            let hook_results = hooks
                .emit(HookEvent::ProviderRequest, json!({"step": step}))
                .await;

            let mut ephemeral: Vec<Value> = Vec::new();
            let mut system_addenda: Vec<String> = Vec::new();
            for result in &hook_results {
                match result {
                    HookResult::InjectContext(text) => {
                        ephemeral.push(json!({"role": "user", "content": text}));
                    }
                    HookResult::SystemPromptAddendum(text) => {
                        system_addenda.push(text.clone());
                    }
                    _ => {}
                }
            }

            // b/c. Get context messages + extend with ephemeral
            let mut msgs = context
                .get_messages_for_request(None, None)
                .await
                .map_err(|e| anyhow::anyhow!("get_messages_for_request failed: {e}"))?;
            msgs.extend(ephemeral);

            // d. Build system prompt
            let mut system_prompt = self.config.system_prompt.clone();
            for addendum in &system_addenda {
                if !system_prompt.is_empty() {
                    system_prompt.push('\n');
                }
                system_prompt.push_str(addendum);
            }

            // e. Convert msgs to Vec<Message> (filter out failures)
            let mut messages: Vec<Message> = msgs
                .into_iter()
                .filter_map(|v| serde_json::from_value::<Message>(v).ok())
                .collect();

            // f. Prepend System message if system_prompt is non-empty
            if !system_prompt.is_empty() {
                let sys_msg = Message {
                    role: Role::System,
                    content: MessageContent::Text(system_prompt),
                    name: None,
                    tool_call_id: None,
                    metadata: None,
                    extensions: HashMap::new(),
                };
                messages.insert(0, sys_msg);
            }

            // g. Build ChatRequest with all fields explicitly set
            let request = ChatRequest {
                messages,
                tools: if tool_specs.is_empty() {
                    None
                } else {
                    Some(tool_specs.clone())
                },
                model: None,
                response_format: None,
                temperature: None,
                top_p: None,
                max_output_tokens: None,
                conversation_id: None,
                stream: None,
                metadata: None,
                tool_choice: None,
                stop: None,
                reasoning_effort: None,
                timeout: None,
                extensions: HashMap::new(),
            };

            // h. Call provider.complete
            let response = provider
                .complete(request)
                .await
                .map_err(|e| anyhow::anyhow!("provider.complete failed: {e}"))?;

            // i. Match finish_reason
            match response.finish_reason.as_deref().unwrap_or("end_turn") {
                "end_turn" | "stop_sequence" | "stop" => {
                    let text = extract_text(&response.content);
                    if !text.is_empty() {
                        on_token(&text);
                    }
                    hooks
                        .emit(HookEvent::TurnEnd, json!({"text": text, "step": step}))
                        .await;
                    self.persist(SessionEvent::Turn {
                        role: "assistant".into(),
                        content: text.clone(),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    })
                    .await;
                    return Ok(text);
                }

                "tool_use" | "tool_calls" => {
                    // Emit preamble text
                    let preamble = extract_text(&response.content);
                    if !preamble.is_empty() {
                        on_token(&preamble);
                    }

                    // Persist assistant message
                    let blocks: Vec<Value> = response
                        .content
                        .iter()
                        .filter_map(|b| serde_json::to_value(b).ok())
                        .collect();
                    let asst_msg = json!({"role": "assistant", "content": blocks});
                    context
                        .add_message(asst_msg)
                        .await
                        .map_err(|e| anyhow::anyhow!("add assistant message failed: {e}"))?;

                    // Parse tool calls
                    let tool_calls = provider.parse_tool_calls(&response);

                    let mut result_blocks: Vec<Value> = Vec::new();
                    for call in &tool_calls {
                        let args_value =
                            serde_json::to_value(&call.arguments).unwrap_or(Value::Null);

                        // Emit ToolPre hook
                        let pre_results = hooks
                            .emit(
                                HookEvent::ToolPre,
                                json!({
                                    "name": call.name,
                                    "id": call.id,
                                    "args": args_value,
                                }),
                            )
                            .await;

                        // Check for Deny
                        let denied = pre_results.iter().find_map(|r| {
                            if let HookResult::Deny(reason) = r {
                                Some(reason.clone())
                            } else {
                                None
                            }
                        });

                        let output = if let Some(reason) = denied {
                            json!(format!("Tool execution denied: {reason}"))
                        } else if let Some(tool) = tools.get(&call.name) {
                            match tool.execute(args_value).await {
                                Ok(result) => result.output.unwrap_or(json!("")),
                                Err(e) => json!(format!("Error: {e}")),
                            }
                        } else {
                            json!(format!("Unknown tool: {}", call.name))
                        };

                        // Emit ToolPost hook
                        hooks
                            .emit(
                                HookEvent::ToolPost,
                                json!({
                                    "name": call.name,
                                    "id": call.id,
                                    "output": output,
                                }),
                            )
                            .await;

                        result_blocks.push(json!({
                            "type": "tool_result",
                            "tool_call_id": call.id,
                            "output": output,
                        }));
                    }

                    // Add tool results to context
                    context
                        .add_message(json!({
                            "role": "user",
                            "content": result_blocks,
                        }))
                        .await
                        .map_err(|e| anyhow::anyhow!("add tool results failed: {e}"))?;
                }

                other => {
                    let text = extract_text(&response.content);
                    if !text.is_empty() {
                        on_token(&text);
                        return Ok(text);
                    }
                    return Err(anyhow::anyhow!("unexpected stop_reason: {other}"));
                }
            }

            step += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract all text from `ContentBlock::Text` blocks and concatenate.
pub fn extract_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::Text { text, .. } = block {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Convert response content blocks into a serialised assistant message Value.
pub fn response_to_message(content: &[ContentBlock]) -> Value {
    let blocks: Vec<Value> = content
        .iter()
        .filter_map(|b| serde_json::to_value(b).ok())
        .collect();
    json!({"role": "assistant", "content": blocks})
}

// ---------------------------------------------------------------------------
// SubagentRunner impl
// ---------------------------------------------------------------------------

#[async_trait]
impl SubagentRunner for LoopOrchestrator {
    async fn resume(
        &self,
        session_id: &str,
        instruction: String,
    ) -> anyhow::Result<amplifier_module_tool_task::SpawnResult> {
        LoopOrchestrator::resume(self, session_id, instruction).await
    }

    async fn run(&self, req: SpawnRequest) -> anyhow::Result<amplifier_module_tool_task::SpawnResult> {
        if req.agent_system_prompt.is_some() || !req.tool_filter.is_empty() {
            // Build a child orchestrator with overridden config.
            // Sub-agents run until they have an answer — do NOT inherit max_steps from the
            // parent, which would artificially cap them at the parent's safety limit.
            let child_config = LoopConfig {
                max_steps: self.config.max_steps, // inherit from parent — matches Python orchestrator_config
                system_prompt: req
                    .agent_system_prompt
                    .unwrap_or_else(|| self.config.system_prompt.clone()),
            };
            let child = LoopOrchestrator::new(child_config);

            // Share providers
            let providers = self.snapshot_providers().await;
            for (name, provider) in providers {
                child.register_provider(name, provider).await;
            }

            // Share tools, filtered by tool_filter if non-empty
            let tools = self.snapshot_tools().await;
            // tool_filter is a DENYLIST — exclude these tools from the child session.
            // (Matches Python: exclude_tools removes named tools from child.)
            let filtered: HashMap<String, Arc<dyn Tool>> = if req.tool_filter.is_empty() {
                tools
            } else {
                tools
                    .into_iter()
                    .filter(|(k, _)| !req.tool_filter.contains(k))
                    .collect()
            };
            log::info!("[subagent] child will have {} tools (excluded: {:?})", filtered.len(), req.tool_filter);
            *child.tools.write().await = filtered;

            // Execute with child orchestrator, tracking every tool call.
            let mut ctx = SimpleContext::new(req.context);
            let mut hooks = HookRegistry::new();
            let tracker = std::sync::Arc::new(ToolTrackerHook::new());
            hooks.register(Box::new(std::sync::Arc::clone(&tracker)));
            let instr_preview = req.instruction.chars().take(60).collect::<String>();
            log::info!("[subagent] calling child.execute() instruction=\"{}…\"", instr_preview);
            let result = child
                .execute(req.instruction, &mut ctx, &hooks, |_| {})
                .await;
            log::info!("[subagent] child.execute() returned ok={}", result.is_ok());
            let tools_called = tracker.get();
            Ok(amplifier_module_tool_task::SpawnResult {
                response: result?,
                session_id: String::new(),
                turn_count: 1,
                tools_called,
            })
        } else {
            // Fall through to self.execute (no customisation needed).
            // Still track tool calls for consistent reporting.
            let mut ctx = SimpleContext::new(req.context);
            let mut hooks = HookRegistry::new();
            let tracker = std::sync::Arc::new(ToolTrackerHook::new());
            hooks.register(Box::new(std::sync::Arc::clone(&tracker)));
            let result = self
                .execute(req.instruction, &mut ctx, &hooks, |_| {})
                .await;
            let tools_called = tracker.get();
            Ok(amplifier_module_tool_task::SpawnResult {
                response: result?,
                session_id: String::new(),
                turn_count: 1,
                tools_called,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use amplifier_core::errors::ProviderError;
    use amplifier_core::messages::{ChatResponse, ToolCall};
    use amplifier_core::models::{ModelInfo, ProviderInfo};
    use std::future::Future;
    use std::pin::Pin;

    // -----------------------------------------------------------------------
    // MockProvider fixture
    // -----------------------------------------------------------------------

    struct MockProvider {
        name: String,
    }

    impl MockProvider {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
            }
        }
    }

    impl Provider for MockProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn get_info(&self) -> ProviderInfo {
            ProviderInfo {
                id: self.name.clone(),
                display_name: self.name.clone(),
                credential_env_vars: vec![],
                capabilities: vec![],
                defaults: HashMap::new(),
                config_fields: vec![],
            }
        }

        fn list_models(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>, ProviderError>> + Send + '_>>
        {
            Box::pin(async move { Ok(vec![]) })
        }

        fn complete(
            &self,
            _request: ChatRequest,
        ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>>
        {
            Box::pin(async move {
                Ok(ChatResponse {
                    content: vec![ContentBlock::Text {
                        text: "mock response".to_string(),
                        visibility: None,
                        extensions: HashMap::new(),
                    }],
                    tool_calls: None,
                    usage: None,
                    degradation: None,
                    finish_reason: Some("end_turn".to_string()),
                    metadata: None,
                    extensions: HashMap::new(),
                })
            })
        }

        fn parse_tool_calls(&self, _response: &ChatResponse) -> Vec<ToolCall> {
            vec![]
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: loop_config_default_max_steps_is_none
    // -----------------------------------------------------------------------

    #[test]
    fn loop_config_default_max_steps_is_none() {
        let config = LoopConfig::default();
        assert_eq!(
            config.max_steps, None,
            "LoopConfig::default() should have max_steps = None (unlimited)"
        );
        assert!(
            config.system_prompt.is_empty(),
            "LoopConfig::default() should have empty system_prompt"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: register_provider_and_snapshot
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn register_provider_and_snapshot() {
        let orchestrator = LoopOrchestrator::new(LoopConfig::default());
        let provider: Arc<dyn Provider> = Arc::new(MockProvider::new("anthropic"));
        orchestrator
            .register_provider("anthropic".to_string(), provider)
            .await;

        let snapshot = orchestrator.snapshot_providers().await;
        assert!(
            snapshot.contains_key("anthropic"),
            "snapshot_providers should contain 'anthropic' after registration"
        );
        assert_eq!(snapshot.len(), 1, "snapshot should have exactly 1 provider");
    }

    // -----------------------------------------------------------------------
    // Test: execute_persists_user_and_assistant_turns
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn execute_persists_user_and_assistant_turns() {
        use amplifier_module_context_simple::SimpleContext;
        use amplifier_module_session_store::{FileSessionStore, SessionEvent, SessionStore};
        let tmp = tempfile::TempDir::new().unwrap();
        let store = std::sync::Arc::new(FileSessionStore::new_with_root(tmp.path().to_path_buf()));
        let session_id = "test-persist-1".to_string();

        let orch = LoopOrchestrator::new(LoopConfig::default());
        orch.attach_store(
            store.clone() as std::sync::Arc<dyn SessionStore>,
            session_id.clone(),
            "test-agent".into(),
            None,
        );
        let provider: std::sync::Arc<dyn Provider> =
            std::sync::Arc::new(MockProvider::new("anthropic"));
        orch.register_provider("anthropic".into(), provider).await;

        store
            .begin(
                &session_id,
                amplifier_module_session_store::SessionMetadata {
                    session_id: session_id.clone(),
                    agent_name: "test-agent".into(),
                    parent_id: None,
                    created: chrono::Utc::now().to_rfc3339(),
                    status: "active".into(),
                },
            )
            .await
            .unwrap();

        let mut ctx = SimpleContext::new(vec![]);
        let hooks = HookRegistry::new();
        orch.execute("hello".to_string(), &mut ctx, &hooks, |_| {})
            .await
            .unwrap();
        orch.finish_store("success").await.unwrap();

        let events = store.load(&session_id).await.unwrap();
        assert!(
            events.len() >= 3,
            "expected session_start + user turn + assistant turn, got {}",
            events.len()
        );
        assert!(events
            .iter()
            .any(|e| matches!(e, SessionEvent::Turn { role, .. } if role == "user")));
        assert!(events
            .iter()
            .any(|e| matches!(e, SessionEvent::Turn { role, .. } if role == "assistant")));
        assert!(matches!(
            events.last().unwrap(),
            SessionEvent::SessionEnd { .. }
        ));
    }

    // -----------------------------------------------------------------------
    // Test: resume_replays_prior_turns_into_context
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resume_replays_prior_turns_into_context() {
        use amplifier_module_session_store::{FileSessionStore, SessionEvent, SessionStore};
        use amplifier_module_tool_task::SubagentRunner;

        let tmp = tempfile::TempDir::new().unwrap();
        let store =
            std::sync::Arc::new(FileSessionStore::new_with_root(tmp.path().to_path_buf()));
        let sid = "resume-1".to_string();

        // Seed a prior completed session
        store
            .begin(
                &sid,
                amplifier_module_session_store::SessionMetadata {
                    session_id: sid.clone(),
                    agent_name: "explorer".into(),
                    parent_id: None,
                    created: "t0".into(),
                    status: "active".into(),
                },
            )
            .await
            .unwrap();
        store
            .append(
                &sid,
                SessionEvent::Turn {
                    role: "user".into(),
                    content: "list rust files".into(),
                    timestamp: "t1".into(),
                },
            )
            .await
            .unwrap();
        store
            .append(
                &sid,
                SessionEvent::Turn {
                    role: "assistant".into(),
                    content: "found: a.rs, b.rs".into(),
                    timestamp: "t2".into(),
                },
            )
            .await
            .unwrap();
        store.finish(&sid, "success", 1).await.unwrap();

        // Create fresh orchestrator (process restart simulation)
        let orch = LoopOrchestrator::new(LoopConfig::default());
        orch.attach_store(
            store.clone() as std::sync::Arc<dyn SessionStore>,
            sid.clone(),
            "explorer".into(),
            None,
        );
        orch.register_provider(
            "anthropic".into(),
            std::sync::Arc::new(MockProvider::new("anthropic")) as std::sync::Arc<dyn Provider>,
        )
        .await;

        let result = SubagentRunner::resume(&orch, &sid, "now count them".to_string())
            .await
            .unwrap();
        assert_eq!(result.session_id, sid);

        let events = store.load(&sid).await.unwrap();
        let user_contents: Vec<&str> = events
            .iter()
            .filter_map(|e| {
                if let SessionEvent::Turn { role, content, .. } = e {
                    if role == "user" {
                        Some(content.as_str())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        assert!(user_contents.iter().any(|c| c.contains("list rust files")));
        assert!(user_contents.iter().any(|c| c.contains("now count them")));
    }

    // -----------------------------------------------------------------------
    // Test 3: extract_text_joins_text_blocks
    // -----------------------------------------------------------------------

    #[test]
    fn extract_text_joins_text_blocks() {
        let content = vec![
            ContentBlock::Text {
                text: "Hello ".to_string(),
                visibility: None,
                extensions: HashMap::new(),
            },
            ContentBlock::ToolCall {
                id: "x".to_string(),
                name: "foo".to_string(),
                input: HashMap::new(),
                visibility: None,
                extensions: HashMap::new(),
            },
            ContentBlock::Text {
                text: "world".to_string(),
                visibility: None,
                extensions: HashMap::new(),
            },
        ];
        assert_eq!(
            extract_text(&content),
            "Hello world",
            "extract_text should join Text blocks and skip non-Text blocks"
        );
    }
}
