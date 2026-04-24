//! Integration tests for LoopOrchestrator.
//!
//! Tests use mock providers/tools with no network calls.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use amplifier_core::errors::{ProviderError, ToolError};
use amplifier_core::messages::{ChatRequest, ChatResponse, ContentBlock, ToolCall, ToolSpec};
use amplifier_core::models::{ModelInfo, ProviderInfo, ToolResult};
use amplifier_core::traits::{ContextManager, Provider, Tool};
use amplifier_module_context_simple::SimpleContext;
use amplifier_module_orchestrator_loop_streaming::{
    Hook, HookContext, HookEvent, HookRegistry, HookResult, LoopConfig, LoopOrchestrator,
};
use async_trait::async_trait;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Fixture: EndTurnProvider
//
// Always returns a ChatResponse with a Text block and finish_reason='end_turn'.
// ---------------------------------------------------------------------------

struct EndTurnProvider {
    response_text: String,
}

impl Provider for EndTurnProvider {
    fn name(&self) -> &str {
        "end_turn_provider"
    }

    fn get_info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "end_turn_provider".to_string(),
            display_name: "EndTurnProvider".to_string(),
            credential_env_vars: vec![],
            capabilities: vec![],
            defaults: HashMap::new(),
            config_fields: vec![],
        }
    }

    fn list_models(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>, ProviderError>> + Send + '_>> {
        Box::pin(async move { Ok(vec![]) })
    }

    fn complete(
        &self,
        _request: ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        let text = self.response_text.clone();
        Box::pin(async move {
            Ok(ChatResponse {
                content: vec![ContentBlock::Text {
                    text,
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

// ---------------------------------------------------------------------------
// Fixture: ToolCallingProvider
//
// Returns ToolCall block with finish_reason='tool_use' while steps_left > 0,
// then returns end_turn once steps are exhausted.
// ---------------------------------------------------------------------------

struct ToolCallingProvider {
    tool_name: String,
    steps_left: Mutex<usize>,
}

impl Provider for ToolCallingProvider {
    fn name(&self) -> &str {
        "tool_calling_provider"
    }

    fn get_info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "tool_calling_provider".to_string(),
            display_name: "ToolCallingProvider".to_string(),
            credential_env_vars: vec![],
            capabilities: vec![],
            defaults: HashMap::new(),
            config_fields: vec![],
        }
    }

    fn list_models(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>, ProviderError>> + Send + '_>> {
        Box::pin(async move { Ok(vec![]) })
    }

    fn complete(
        &self,
        _request: ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        // Acquire and release the lock synchronously before creating the async future.
        let should_call_tool = {
            let mut steps = self.steps_left.lock().unwrap();
            if *steps > 0 {
                *steps -= 1;
                true
            } else {
                false
            }
        };
        let tool_name = self.tool_name.clone();
        Box::pin(async move {
            if should_call_tool {
                Ok(ChatResponse {
                    content: vec![ContentBlock::ToolCall {
                        id: "call_1".to_string(),
                        name: tool_name,
                        input: HashMap::new(),
                        visibility: None,
                        extensions: HashMap::new(),
                    }],
                    tool_calls: None,
                    usage: None,
                    degradation: None,
                    finish_reason: Some("tool_use".to_string()),
                    metadata: None,
                    extensions: HashMap::new(),
                })
            } else {
                Ok(ChatResponse {
                    content: vec![ContentBlock::Text {
                        text: "done".to_string(),
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
            }
        })
    }

    fn parse_tool_calls(&self, response: &ChatResponse) -> Vec<ToolCall> {
        response
            .content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolCall {
                    id, name, input, ..
                } = block
                {
                    Some(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: input.clone(),
                        extensions: HashMap::new(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Fixture: CountingProvider
//
// Records req.messages.len() into a shared Mutex<usize> on each complete()
// call, and returns end_turn.
// ---------------------------------------------------------------------------

struct CountingProvider {
    msg_count: Arc<Mutex<usize>>,
}

impl Provider for CountingProvider {
    fn name(&self) -> &str {
        "counting_provider"
    }

    fn get_info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "counting_provider".to_string(),
            display_name: "CountingProvider".to_string(),
            credential_env_vars: vec![],
            capabilities: vec![],
            defaults: HashMap::new(),
            config_fields: vec![],
        }
    }

    fn list_models(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>, ProviderError>> + Send + '_>> {
        Box::pin(async move { Ok(vec![]) })
    }

    fn complete(
        &self,
        request: ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        let count = request.messages.len();
        let msg_count = self.msg_count.clone();
        Box::pin(async move {
            *msg_count.lock().unwrap() = count;
            Ok(ChatResponse {
                content: vec![ContentBlock::Text {
                    text: "done".to_string(),
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

// ---------------------------------------------------------------------------
// Fixture: EchoTool
//
// Tool impl returning 'echo ok'.
// ---------------------------------------------------------------------------

struct EchoTool {
    name: String,
}

impl Tool for EchoTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Echoes 'echo ok' back"
    }

    fn get_spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            parameters: HashMap::new(),
            description: Some("Echoes 'echo ok' back".to_string()),
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        _input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(async move {
            Ok(ToolResult {
                success: true,
                output: Some(json!("echo ok")),
                error: None,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Fixture: DenyAll hook
//
// Subscribes to ToolPre and always returns Deny.
// ---------------------------------------------------------------------------

struct DenyAll;

#[async_trait]
impl Hook for DenyAll {
    fn events(&self) -> &[HookEvent] {
        &[HookEvent::ToolPre]
    }

    async fn handle(&self, _ctx: &HookContext) -> HookResult {
        HookResult::Deny("test denial".to_string())
    }
}

// ---------------------------------------------------------------------------
// Fixture: InjectHook
//
// Subscribes to ProviderRequest and returns InjectContext with the given text.
// ---------------------------------------------------------------------------

struct InjectHook {
    text: String,
}

#[async_trait]
impl Hook for InjectHook {
    fn events(&self) -> &[HookEvent] {
        &[HookEvent::ProviderRequest]
    }

    async fn handle(&self, _ctx: &HookContext) -> HookResult {
        HookResult::InjectContext(self.text.clone())
    }
}

// ---------------------------------------------------------------------------
// Test A: execute_returns_provider_text_on_end_turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_returns_provider_text_on_end_turn() {
    let orchestrator = LoopOrchestrator::new(LoopConfig::default());
    let provider: Arc<dyn Provider> = Arc::new(EndTurnProvider {
        response_text: "The answer is 42.".to_string(),
    });
    orchestrator
        .register_provider("anthropic".to_string(), provider)
        .await;

    let mut ctx = SimpleContext::new(vec![]);
    let hooks = HookRegistry::new();

    // Capture tokens via shared state (on_token must be Send).
    let received_tokens: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let received_tokens_clone = received_tokens.clone();

    let result = orchestrator
        .execute("What is 6×7?".to_string(), &mut ctx, &hooks, move |token| {
            received_tokens_clone
                .lock()
                .unwrap()
                .push(token.to_string());
        })
        .await;

    assert!(result.is_ok(), "Expected Ok result, got: {:?}", result);
    let text = result.unwrap();
    assert_eq!(
        text, "The answer is 42.",
        "Result text must match provider response"
    );

    let tokens = received_tokens.lock().unwrap();
    assert!(
        !tokens.is_empty(),
        "on_token callback should have been called at least once"
    );
    assert!(
        tokens.iter().any(|t| t.contains("42")),
        "on_token should have received the response text, got: {:?}",
        tokens
    );
}

// ---------------------------------------------------------------------------
// Test B: execute_adds_user_message_to_context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_adds_user_message_to_context() {
    let orchestrator = LoopOrchestrator::new(LoopConfig::default());
    let provider: Arc<dyn Provider> = Arc::new(EndTurnProvider {
        response_text: "pong".to_string(),
    });
    orchestrator
        .register_provider("anthropic".to_string(), provider)
        .await;

    let mut ctx = SimpleContext::new(vec![]);
    let hooks = HookRegistry::new();

    orchestrator
        .execute("ping".to_string(), &mut ctx, &hooks, |_| {})
        .await
        .unwrap();

    let messages = ctx.get_messages().await.unwrap();
    let has_ping = messages.iter().any(|m| {
        m.get("content")
            .and_then(|c| c.as_str())
            .map(|s| s.contains("ping"))
            .unwrap_or(false)
    });
    assert!(
        has_ping,
        "Context should contain user message 'ping', got: {:?}",
        messages
    );
}

// ---------------------------------------------------------------------------
// Test C: execute_enforces_max_steps
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_enforces_max_steps() {
    let orchestrator = LoopOrchestrator::new(LoopConfig {
        max_steps: 3,
        system_prompt: String::new(),
    });

    // Provider that will never stop calling tools.
    let provider: Arc<dyn Provider> = Arc::new(ToolCallingProvider {
        tool_name: "echo".to_string(),
        steps_left: Mutex::new(999),
    });
    orchestrator
        .register_provider("anthropic".to_string(), provider)
        .await;

    // Register EchoTool so the loop actually executes the tool each step.
    let tool: Arc<dyn Tool> = Arc::new(EchoTool {
        name: "echo".to_string(),
    });
    orchestrator.register_tool(tool).await;

    let mut ctx = SimpleContext::new(vec![]);
    let hooks = HookRegistry::new();

    let result = orchestrator
        .execute("run forever".to_string(), &mut ctx, &hooks, |_| {})
        .await;

    assert!(
        result.is_err(),
        "Expected Err result when max_steps exceeded, got: {:?}",
        result
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("max_steps") || err_msg.contains("3"),
        "Error message should mention 'max_steps' or '3', got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// Test D: tool_pre_deny_prevents_tool_execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tool_pre_deny_prevents_tool_execution() {
    let orchestrator = LoopOrchestrator::new(LoopConfig::default());

    // Provider that makes exactly 1 tool call, then returns end_turn.
    let provider: Arc<dyn Provider> = Arc::new(ToolCallingProvider {
        tool_name: "echo".to_string(),
        steps_left: Mutex::new(1),
    });
    orchestrator
        .register_provider("anthropic".to_string(), provider)
        .await;

    let tool: Arc<dyn Tool> = Arc::new(EchoTool {
        name: "echo".to_string(),
    });
    orchestrator.register_tool(tool).await;

    let mut ctx = SimpleContext::new(vec![]);
    let mut hooks = HookRegistry::new();
    hooks.register(Box::new(DenyAll));

    let result = orchestrator
        .execute("test".to_string(), &mut ctx, &hooks, |_| {})
        .await;

    assert!(
        result.is_ok(),
        "DenyAll hook should skip tool gracefully without aborting the loop, got: {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// Test E: provider_request_hook_inject_context_is_included_in_messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_request_hook_inject_context_is_included_in_messages() {
    let msg_count = Arc::new(Mutex::new(0usize));

    let orchestrator = LoopOrchestrator::new(LoopConfig::default());
    let provider: Arc<dyn Provider> = Arc::new(CountingProvider {
        msg_count: msg_count.clone(),
    });
    orchestrator
        .register_provider("anthropic".to_string(), provider)
        .await;

    let mut ctx = SimpleContext::new(vec![]);
    let mut hooks = HookRegistry::new();
    hooks.register(Box::new(InjectHook {
        text: "extra context injected".to_string(),
    }));

    orchestrator
        .execute("test".to_string(), &mut ctx, &hooks, |_| {})
        .await
        .unwrap();

    // Provider should have seen at least 2 messages: the user message + the injected ephemeral.
    let count = *msg_count.lock().unwrap();
    assert!(
        count >= 2,
        "Provider should see at least 2 messages (user + ephemeral injection), got: {}",
        count
    );

    // The ephemeral injection must NOT appear in ctx.get_messages() (persistent history).
    let history = ctx.get_messages().await.unwrap();
    let has_ephemeral = history.iter().any(|m| {
        m.get("content")
            .and_then(|c| c.as_str())
            .map(|s| s.contains("extra context injected"))
            .unwrap_or(false)
    });
    assert!(
        !has_ephemeral,
        "Ephemeral injection must not leak into context history, but found it in: {:?}",
        history
    );
}
