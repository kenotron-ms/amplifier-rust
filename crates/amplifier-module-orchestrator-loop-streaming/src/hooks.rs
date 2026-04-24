//! Hook system: typed lifecycle events, results, and a dispatch registry.

use async_trait::async_trait;
use serde_json::Value;

// ---------------------------------------------------------------------------
// HookEvent
// ---------------------------------------------------------------------------

/// Lifecycle events emitted by the orchestrator loop.
#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    /// Fired once at session start, before the first user message is added.
    SessionStart,
    /// Fired before each LLM call. Hooks may inject ephemeral context.
    ProviderRequest,
    /// Fired before a tool executes. Hooks may deny execution.
    ToolPre,
    /// Fired after a tool returns. Hooks receive the tool result.
    ToolPost,
    /// Fired after a complete turn (end_turn signal).
    TurnEnd,
}

// ---------------------------------------------------------------------------
// HookContext
// ---------------------------------------------------------------------------

/// Context passed to every `Hook::handle()` call.
pub struct HookContext {
    /// The lifecycle event that triggered this call.
    pub event: HookEvent,
    /// Arbitrary JSON payload associated with the event.
    pub data: Value,
}

// ---------------------------------------------------------------------------
// HookResult
// ---------------------------------------------------------------------------

/// The outcome returned by a hook after handling an event.
#[derive(Debug, Clone, PartialEq)]
pub enum HookResult {
    /// No special action; the orchestrator continues normally.
    Continue,
    /// Append the given string to the system prompt for this turn only.
    SystemPromptAddendum(String),
    /// Inject the given string as an ephemeral user message.
    /// The injected message is cleared after the next provider call.
    InjectContext(String),
    /// Deny the pending operation (only valid from `ToolPre`).
    Deny(String),
}

// ---------------------------------------------------------------------------
// Hook trait
// ---------------------------------------------------------------------------

/// A lifecycle hook that may observe and influence orchestrator behaviour.
///
/// # Contract
///
/// * `events()` declares which [`HookEvent`] variants this hook wants to
///   receive.  The registry will only call `handle()` for listed events.
/// * `handle()` must not panic.
/// * Implementations must be `Send + Sync`.
#[async_trait]
pub trait Hook: Send + Sync {
    /// Return the set of lifecycle events this hook subscribes to.
    fn events(&self) -> &[HookEvent];

    /// Handle a single lifecycle event.
    async fn handle(&self, ctx: &HookContext) -> HookResult;
}

// ---------------------------------------------------------------------------
// HookRegistry
// ---------------------------------------------------------------------------

/// Holds registered hooks and dispatches lifecycle events.
///
/// Hooks fire in registration order. Each hook receives only the events it
/// listed in `events()`.
#[derive(Default)]
pub struct HookRegistry {
    hooks: Vec<Box<dyn Hook>>,
}

impl HookRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a hook.
    ///
    /// Hooks are appended to the end of the list, so they fire in registration order.
    pub fn register(&mut self, hook: Box<dyn Hook>) {
        self.hooks.push(hook);
    }

    /// Emit a lifecycle event and collect results from all subscribed hooks.
    ///
    /// Hooks are called sequentially in registration order. A hook is skipped
    /// if the emitted event is not in its `events()` list.
    pub async fn emit(&self, event: HookEvent, data: Value) -> Vec<HookResult> {
        let ctx = HookContext {
            event: event.clone(),
            data,
        };
        let mut results = Vec::new();
        for hook in &self.hooks {
            if hook.events().contains(&ctx.event) {
                results.push(hook.handle(&ctx).await);
            }
        }
        results
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Fixtures
    // -----------------------------------------------------------------------

    /// A hook that counts how many times `handle()` is called.
    struct CountingHook {
        subscribed: Vec<HookEvent>,
        count: Arc<AtomicUsize>,
    }

    impl CountingHook {
        fn new(subscribed: Vec<HookEvent>) -> (Self, Arc<AtomicUsize>) {
            let count = Arc::new(AtomicUsize::new(0));
            let hook = Self {
                subscribed,
                count: count.clone(),
            };
            (hook, count)
        }
    }

    #[async_trait]
    impl Hook for CountingHook {
        fn events(&self) -> &[HookEvent] {
            &self.subscribed
        }

        async fn handle(&self, _ctx: &HookContext) -> HookResult {
            self.count.fetch_add(1, Ordering::SeqCst);
            HookResult::Continue
        }
    }

    /// A hook that always returns `HookResult::InjectContext`.
    struct InjectingHook {
        subscribed: Vec<HookEvent>,
    }

    impl InjectingHook {
        fn new(subscribed: Vec<HookEvent>) -> Self {
            Self { subscribed }
        }
    }

    #[async_trait]
    impl Hook for InjectingHook {
        fn events(&self) -> &[HookEvent] {
            &self.subscribed
        }

        async fn handle(&self, _ctx: &HookContext) -> HookResult {
            HookResult::InjectContext("injected context".to_string())
        }
    }

    /// A hook that always returns `HookResult::Deny`.
    struct DenyingHook {
        subscribed: Vec<HookEvent>,
    }

    impl DenyingHook {
        fn new(subscribed: Vec<HookEvent>) -> Self {
            Self { subscribed }
        }
    }

    #[async_trait]
    impl Hook for DenyingHook {
        fn events(&self) -> &[HookEvent] {
            &self.subscribed
        }

        async fn handle(&self, _ctx: &HookContext) -> HookResult {
            HookResult::Deny("denied by hook".to_string())
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn empty_registry_returns_empty_results() {
        let registry = HookRegistry::new();
        let results = registry
            .emit(HookEvent::ToolPre, serde_json::json!({}))
            .await;
        assert!(
            results.is_empty(),
            "empty registry should produce no results, got: {results:?}"
        );
    }

    #[tokio::test]
    async fn hook_only_receives_subscribed_events() {
        let mut registry = HookRegistry::new();
        let (hook, count) = CountingHook::new(vec![HookEvent::ToolPre]);
        registry.register(Box::new(hook));

        // Emit an event the hook is NOT subscribed to.
        let results = registry
            .emit(HookEvent::ToolPost, serde_json::json!({}))
            .await;

        assert!(
            results.is_empty(),
            "hook subscribed only to ToolPre should not fire for ToolPost"
        );
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "call count should remain 0 for non-subscribed event"
        );
    }

    #[tokio::test]
    async fn hook_fires_for_subscribed_event() {
        let mut registry = HookRegistry::new();
        let (hook, count) = CountingHook::new(vec![HookEvent::ToolPre]);
        registry.register(Box::new(hook));

        let results = registry
            .emit(HookEvent::ToolPre, serde_json::json!({}))
            .await;

        assert_eq!(
            results.len(),
            1,
            "one result expected for one subscribed hook"
        );
        assert_eq!(results[0], HookResult::Continue);
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "hook should have been called exactly once"
        );
    }

    #[tokio::test]
    async fn emit_returns_inject_context_result() {
        let mut registry = HookRegistry::new();
        registry.register(Box::new(InjectingHook::new(vec![
            HookEvent::ProviderRequest,
        ])));

        let results = registry
            .emit(HookEvent::ProviderRequest, serde_json::json!({}))
            .await;

        assert_eq!(results.len(), 1);
        assert!(
            matches!(&results[0], HookResult::InjectContext(s) if !s.is_empty()),
            "expected InjectContext result, got: {:?}",
            results[0]
        );
    }

    #[tokio::test]
    async fn emit_returns_deny_result() {
        let mut registry = HookRegistry::new();
        registry.register(Box::new(DenyingHook::new(vec![HookEvent::ToolPre])));

        let results = registry
            .emit(HookEvent::ToolPre, serde_json::json!({}))
            .await;

        assert_eq!(results.len(), 1);
        assert!(
            matches!(&results[0], HookResult::Deny(s) if !s.is_empty()),
            "expected Deny result, got: {:?}",
            results[0]
        );
    }

    #[tokio::test]
    async fn multiple_hooks_all_fire() {
        let mut registry = HookRegistry::new();
        let (hook1, count1) = CountingHook::new(vec![HookEvent::TurnEnd]);
        let (hook2, count2) = CountingHook::new(vec![HookEvent::TurnEnd]);
        let (hook3, count3) = CountingHook::new(vec![HookEvent::TurnEnd]);
        registry.register(Box::new(hook1));
        registry.register(Box::new(hook2));
        registry.register(Box::new(hook3));

        let results = registry
            .emit(HookEvent::TurnEnd, serde_json::json!({}))
            .await;

        assert_eq!(
            results.len(),
            3,
            "all three registered hooks should have fired"
        );
        assert_eq!(count1.load(Ordering::SeqCst), 1, "hook1 should fire once");
        assert_eq!(count2.load(Ordering::SeqCst), 1, "hook2 should fire once");
        assert_eq!(count3.load(Ordering::SeqCst), 1, "hook3 should fire once");
        assert!(results.iter().all(|r| *r == HookResult::Continue));
    }
}
