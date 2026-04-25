//! Foundation context for amplifier-rust.
//!
//! Ports the Python `amplifier-foundation` bundle's context injection system to Rust.
//! The foundation bundle teaches the LLM to delegate autonomously to specialist agents
//! rather than attempting complex tasks directly.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use amplifier_context_foundation::FoundationContextHook;
//! use amplifier_module_orchestrator_loop_streaming::HookRegistry;
//!
//! let mut hooks = HookRegistry::new();
//! hooks.register(Box::new(FoundationContextHook::new()));
//! ```
//!
//! The hook fires on every `ProviderRequest` event, injecting the delegation
//! instructions and multi-agent patterns as ephemeral context before each LLM call.

use amplifier_module_orchestrator_loop_streaming::{Hook, HookContext, HookEvent, HookResult};

// ---------------------------------------------------------------------------
// Embedded context files (exact ports from Python amplifier-foundation)
// ---------------------------------------------------------------------------

/// Delegation instructions — teaches the LLM to delegate to agents instead of
/// doing complex work directly. Mirrors `foundation:context/agents/delegation-instructions.md`.
pub const DELEGATION_INSTRUCTIONS: &str =
    include_str!("../context/delegation-instructions.md");

/// Multi-agent patterns — teaches parallel dispatch, context sharing, session
/// resumption. Mirrors `foundation:context/agents/multi-agent-patterns.md`.
pub const MULTI_AGENT_PATTERNS: &str =
    include_str!("../context/multi-agent-patterns.md");

// ---------------------------------------------------------------------------
// FoundationContextHook
// ---------------------------------------------------------------------------

/// Hook that injects foundation context before each LLM provider call.
///
/// Mirrors the Python `amplifier-foundation` bundle's context injection —
/// the same text that teaches the LLM to delegate autonomously in Python
/// sessions is now injected in Rust sessions too.
///
/// Fires on `HookEvent::ProviderRequest`.
pub struct FoundationContextHook {
    /// Cached combined context string (built once, injected every turn).
    context: String,
}

impl FoundationContextHook {
    /// Create a hook that injects all foundation context files.
    pub fn new() -> Self {
        let context = format!(
            "{}\n\n---\n\n{}",
            DELEGATION_INSTRUCTIONS, MULTI_AGENT_PATTERNS,
        );
        Self { context }
    }

    /// Create a hook with custom context content appended to the foundation files.
    ///
    /// Use this to add project-specific context alongside the foundation files.
    pub fn with_extra(extra: &str) -> Self {
        let context = format!(
            "{}\n\n---\n\n{}\n\n---\n\n{}",
            DELEGATION_INSTRUCTIONS, MULTI_AGENT_PATTERNS, extra,
        );
        Self { context }
    }
}

impl Default for FoundationContextHook {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Hook for FoundationContextHook {
    fn events(&self) -> &[HookEvent] {
        &[HookEvent::ProviderRequest]
    }

    async fn handle(&self, _ctx: &HookContext) -> HookResult {
        HookResult::InjectContext(self.context.clone())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use amplifier_module_orchestrator_loop_streaming::HookEvent;

    #[test]
    fn delegation_instructions_non_empty_and_has_key_concepts() {
        assert!(!DELEGATION_INSTRUCTIONS.is_empty());
        assert!(
            DELEGATION_INSTRUCTIONS.contains("ORCHESTRATOR"),
            "must contain the orchestrator framing"
        );
        assert!(
            DELEGATION_INSTRUCTIONS.contains("explorer"),
            "must reference explorer agent"
        );
        assert!(
            DELEGATION_INSTRUCTIONS.contains("delegate"),
            "must reference the delegate tool"
        );
    }

    #[test]
    fn multi_agent_patterns_non_empty_and_has_key_concepts() {
        assert!(!MULTI_AGENT_PATTERNS.is_empty());
        assert!(
            MULTI_AGENT_PATTERNS.contains("Parallel"),
            "must cover parallel dispatch"
        );
        assert!(
            MULTI_AGENT_PATTERNS.contains("context_scope"),
            "must cover context sharing"
        );
    }

    #[test]
    fn hook_new_combines_both_files() {
        let hook = FoundationContextHook::new();
        assert!(hook.context.contains("ORCHESTRATOR"));
        assert!(hook.context.contains("Parallel Agent Dispatch"));
    }

    #[test]
    fn hook_with_extra_appends_custom_content() {
        let hook = FoundationContextHook::with_extra("## Custom Context\nproject-specific");
        assert!(hook.context.contains("ORCHESTRATOR"));
        assert!(hook.context.contains("project-specific"));
    }

    #[tokio::test]
    async fn hook_returns_inject_context_on_provider_request() {
        let hook = FoundationContextHook::new();
        assert_eq!(hook.events(), &[HookEvent::ProviderRequest]);
        // HookContext has no Default impl — construct explicitly.
        let ctx = HookContext {
            event: HookEvent::ProviderRequest,
            data: serde_json::json!({}),
        };
        let result = hook.handle(&ctx).await;
        match result {
            HookResult::InjectContext(text) => {
                assert!(text.contains("ORCHESTRATOR"));
                assert!(text.contains("Parallel"));
            }
            other => panic!("expected InjectContext, got {:?}", other),
        }
    }
}
