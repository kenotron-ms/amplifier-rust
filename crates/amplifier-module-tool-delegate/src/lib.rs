//! amplifier-module-tool-delegate — delegate tool for spawning named sub-agents.
//!
//! This crate provides the [`DelegateTool`] which enables agents to delegate
//! work to named sub-agents from an [`AgentRegistry`].

pub mod context;
pub mod resolver;

use std::sync::Arc;

use amplifier_module_agent_runtime::AgentRegistry;

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
    #[allow(dead_code)]
    runner: Arc<dyn SubagentRunner>,
    #[allow(dead_code)]
    registry: Arc<AgentRegistry>,
    #[allow(dead_code)]
    config: DelegateConfig,
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
        assert!(id.starts_with("parent-"), "expected 'parent-' prefix, got: {id}");
        assert!(id.ends_with("_explorer"), "expected '_explorer' suffix, got: {id}");
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
}
