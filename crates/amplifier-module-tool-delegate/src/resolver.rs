//! Agent resolver stub — resolves agent names to configs.

use amplifier_module_agent_runtime::{AgentConfig, AgentRegistry};

/// Resolution outcome for an agent lookup.
pub enum ResolvedAgent {
    /// The requested agent is "self" — run the same orchestrator recursively.
    SelfDelegate,
    /// A named agent was found in the registry.
    FoundAgent(AgentConfig),
}

/// Resolve an agent by name from the registry.
///
/// # Errors
/// Always returns `Err("not implemented")` in this stub.
pub fn resolve_agent(_name: &str, _registry: &AgentRegistry) -> anyhow::Result<ResolvedAgent> {
    anyhow::bail!("not implemented")
}
