//! Agent runtime — AgentConfig, AgentRegistry, and bundle loading.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// AgentConfig
// ---------------------------------------------------------------------------

/// Configuration for a single agent bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Unique agent name (used as the registry key).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Tool name allowlist. Empty vec means inherit all available tools.
    pub tools: Vec<String>,
    /// System prompt for this agent.
    pub instruction: String,
}

// ---------------------------------------------------------------------------
// AgentRegistry
// ---------------------------------------------------------------------------

/// In-memory registry that maps agent names to their [`AgentConfig`].
pub struct AgentRegistry {
    agents: HashMap<String, AgentConfig>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Add (or replace) an agent in the registry.
    pub fn register(&mut self, config: AgentConfig) {
        self.agents.insert(config.name.clone(), config);
    }

    /// Look up an agent by name.
    pub fn get(&self, name: &str) -> Option<&AgentConfig> {
        self.agents.get(name)
    }

    /// Return all registered agents, sorted by name.
    pub fn list(&self) -> Vec<&AgentConfig> {
        let mut configs: Vec<&AgentConfig> = self.agents.values().collect();
        configs.sort_by(|a, b| a.name.cmp(&b.name));
        configs
    }

    /// Return all registered agent names, sorted.
    pub fn available_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.agents.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_register_and_get() {
        let mut registry = AgentRegistry::new();
        registry.register(AgentConfig {
            name: "my-agent".to_string(),
            description: "A test agent".to_string(),
            tools: vec!["bash".to_string()],
            instruction: "You are a test agent.".to_string(),
        });
        let found = registry.get("my-agent").expect("should find agent");
        assert_eq!(found.name, "my-agent");
        assert_eq!(found.instruction, "You are a test agent.");
    }

    #[test]
    fn registry_list_is_sorted() {
        let mut registry = AgentRegistry::new();
        for name in &["zebra", "alpha", "mango"] {
            registry.register(AgentConfig {
                name: name.to_string(),
                description: String::new(),
                tools: vec![],
                instruction: String::new(),
            });
        }
        let names: Vec<&str> = registry.list().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mango", "zebra"]);
    }
}
