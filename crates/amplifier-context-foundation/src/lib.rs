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
//! The hook fires on every `ProviderRequest` event, injecting the compact
//! delegate-first context as ephemeral context before each LLM call.

use amplifier_module_orchestrator_loop_streaming::{Hook, HookContext, HookEvent, HookResult};

// ---------------------------------------------------------------------------
// Lean delegation context — compact replacement for large context files
// ---------------------------------------------------------------------------

/// Exp-lean delegation context — injected once per ProviderRequest.
/// Compact version of foundation context: establishes delegate-first mindset
/// without bloating every turn with thousands of tokens of philosophy.
/// Agent list is already in DelegateTool.get_spec() — not repeated here.
const EXP_LEAN_CONTEXT: &str = "\
## Delegate-First Operation

DEFAULT: delegate to specialist agents. EXCEPTION: trivial single-step ops only.\n\
Use the `delegate` tool for: file exploration (>2 files), debugging, implementation, \
architecture decisions, git operations, security review.\n\

## Skill Discipline

**If there is even a 1% chance a skill applies to what you are doing, you MUST load it \
before responding.** Skills carry all behavioral guidance — they are not optional.\n\

Available skills (use load_skill tool before acting on relevant tasks):\n\
- **using-superpowers**: Load at the start of any conversation to establish skill-first discipline\n\
- **brainstorming**: MUST load before any new feature, component, or creative work\n\
- **systematic-debugging**: Load before fixing any bug or unexpected behavior\n\
- **test-driven-development**: Load before writing implementation code\n\
- **writing-plans**: Load before multi-step implementation tasks\n\
- **verification-before-completion**: Load before claiming work is done or committing\n\
- **dispatching-parallel-agents**: Load when 2+ independent tasks can run in parallel\n\
- **subagent-driven-development**: Load when executing implementation plans with sub-agents\n\
- **executing-plans**: Load when you have a written plan to execute\n\

**Relay**: always summarize agent/tool results in your final message — \
the user sees only your final response text, not intermediate tool output.\
";

// Old constants kept on disk (context/*.md) but no longer embedded here:
// pub const DELEGATION_INSTRUCTIONS: &str = include_str!("../context/delegation-instructions.md");
// pub const MULTI_AGENT_PATTERNS: &str    = include_str!("../context/multi-agent-patterns.md");

// ---------------------------------------------------------------------------
// FoundationContextHook
// ---------------------------------------------------------------------------

/// Hook that injects foundation context before each LLM provider call.
///
/// Uses a compact inline context (~400 chars) instead of the full markdown
/// files, reducing per-turn token overhead while preserving the delegate-first
/// behavioral contract.
///
/// Fires on `HookEvent::ProviderRequest`.
pub struct FoundationContextHook {
    /// Cached combined context string (built once, injected every turn).
    context: String,
}

impl FoundationContextHook {
    /// Create a hook that injects the lean foundation context.
    pub fn new() -> Self {
        Self {
            context: EXP_LEAN_CONTEXT.to_string(),
        }
    }

    /// Create a hook with custom context content appended after the lean foundation context.
    ///
    /// Use this to add project-specific context alongside the foundation context.
    pub fn with_extra(extra: &str) -> Self {
        let context = format!("{}\n\n---\n\n{}", EXP_LEAN_CONTEXT, extra);
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
        // SystemPromptAddendum appends to the system prompt (high-weight).
        // InjectContext injects as a user-role message (low-weight, ignored by the LLM).
        HookResult::SystemPromptAddendum(self.context.clone())
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
    fn lean_context_non_empty_and_has_key_concepts() {
        assert!(!EXP_LEAN_CONTEXT.is_empty());
        assert!(
            EXP_LEAN_CONTEXT.contains("delegate"),
            "must reference the delegate tool"
        );
        assert!(
            EXP_LEAN_CONTEXT.contains("load_skill"),
            "must reference load_skill for on-demand skills"
        );
        assert!(
            EXP_LEAN_CONTEXT.contains("Delegate-First"),
            "must contain the delegate-first framing"
        );
    }

    #[test]
    fn hook_new_uses_lean_context() {
        let hook = FoundationContextHook::new();
        assert!(hook.context.contains("Delegate-First"));
        assert!(hook.context.contains("load_skill"));
        assert!(hook.context.contains("delegate"));
    }

    #[test]
    fn hook_with_extra_appends_custom_content() {
        let hook = FoundationContextHook::with_extra("## Custom Context\nproject-specific");
        assert!(hook.context.contains("Delegate-First"));
        assert!(hook.context.contains("project-specific"));
    }

    #[tokio::test]
    async fn hook_returns_system_prompt_addendum_on_provider_request() {
        let hook = FoundationContextHook::new();
        assert_eq!(hook.events(), &[HookEvent::ProviderRequest]);
        // HookContext has no Default impl — construct explicitly.
        let ctx = HookContext {
            event: HookEvent::ProviderRequest,
            data: serde_json::json!({}),
        };
        let result = hook.handle(&ctx).await;
        match result {
            HookResult::SystemPromptAddendum(text) => {
                assert!(text.contains("Delegate-First"));
                assert!(text.contains("load_skill"));
            }
            other => panic!("expected SystemPromptAddendum, got {:?}", other),
        }
    }
}
