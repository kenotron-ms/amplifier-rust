//! LoopOrchestrator — streaming agent loop with hooks, step limit, and tool dispatch.

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
use amplifier_module_tool_task::{SpawnRequest, SubagentRunner};

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
    pub config: LoopConfig,
    pub providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
    pub tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
}

impl LoopOrchestrator {
    /// Create a new orchestrator with the given configuration.
    pub fn new(config: LoopConfig) -> Self {
        Self {
            config,
            providers: RwLock::new(HashMap::new()),
            tools: RwLock::new(HashMap::new()),
        }
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
    async fn run(&self, req: SpawnRequest) -> anyhow::Result<String> {
        let mut ctx = SimpleContext::new(req.context);
        let hooks = HookRegistry::new();
        self.execute(req.instruction, &mut ctx, &hooks, |_| {})
            .await
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
