//! Hooks for the amplifier-android-sandbox.
//!
//! This sandbox has no Kotlin side, so hooks are pure Rust.  `LoggingHook` replaces
//! `StatusContextHook` for local dev — it prints every lifecycle event to stderr and
//! returns `HookResult::Continue` without altering orchestrator behaviour.

use amplifier_module_orchestrator_loop_streaming::{
    Hook, HookContext, HookEvent, HookRegistry, HookResult,
};
use async_trait::async_trait;

// ---------------------------------------------------------------------------
// LoggingHook
// ---------------------------------------------------------------------------

/// A pure-Rust hook that logs every orchestrator lifecycle event to stderr.
///
/// Subscribes to all five events:
/// [`HookEvent::SessionStart`], [`HookEvent::ProviderRequest`],
/// [`HookEvent::ToolPre`], [`HookEvent::ToolPost`], [`HookEvent::TurnEnd`].
///
/// Returns [`HookResult::Continue`] unconditionally so it never alters
/// orchestrator behaviour.
pub struct LoggingHook;

#[async_trait]
impl Hook for LoggingHook {
    fn events(&self) -> &[HookEvent] {
        &[
            HookEvent::SessionStart,
            HookEvent::ProviderRequest,
            HookEvent::ToolPre,
            HookEvent::ToolPost,
            HookEvent::TurnEnd,
        ]
    }

    async fn handle(&self, ctx: &HookContext) -> HookResult {
        eprintln!("[hook] {:?}: {}", ctx.event, ctx.data);
        HookResult::Continue
    }
}

// ---------------------------------------------------------------------------
// build_registry
// ---------------------------------------------------------------------------

/// Build a [`HookRegistry`] pre-populated with the sandbox hooks.
///
/// Currently registers [`LoggingHook`] which observes all five lifecycle
/// events and emits debug output to stderr.
pub fn build_registry() -> HookRegistry {
    let mut r = HookRegistry::new();
    r.register(Box::new(LoggingHook));
    r
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_builds_without_panic() {
        let registry = build_registry();
        let results = registry
            .emit(HookEvent::SessionStart, serde_json::json!({}))
            .await;
        assert!(
            !results.is_empty(),
            "registry should contain at least one hook"
        );
    }
}
