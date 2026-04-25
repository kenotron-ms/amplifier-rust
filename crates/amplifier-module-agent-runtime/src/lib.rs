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

    /// Load all agent bundle files from `dir` into the registry.
    ///
    /// Scans `dir` for `.md` files with YAML frontmatter and loads each one.
    /// Non-existent directories are not an error — they return `Ok(0)`.
    ///
    /// # Format
    /// ```text
    /// ---
    /// meta:
    ///   name: my-agent
    ///   description: Does things
    /// tools:
    ///   - bash
    ///   - filesystem
    /// ---
    /// System prompt here.
    /// ```
    pub fn load_from_dir(&mut self, dir: &std::path::Path) -> anyhow::Result<usize> {
        if !dir.is_dir() {
            return Ok(0);
        }

        let entries = std::fs::read_dir(dir)
            .map_err(|e| anyhow::anyhow!("failed to read directory {}: {}", dir.display(), e))?;

        let mut count = 0;

        for entry_result in entries {
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            // Only process .md files.
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }

            // Read file content — skip silently on failure.
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Parse the agent file — skip silently on error.
            if let Some(config) = parse_agent_file(&content) {
                self.register(config);
                count += 1;
            }
        }

        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Minimal agent file parser
// ---------------------------------------------------------------------------

/// Parse YAML frontmatter from an agent bundle `.md` file into an [`AgentConfig`].
///
/// Returns `None` if the file cannot be parsed (missing frontmatter delimiters,
/// missing `meta.name`, or YAML parse error).
fn parse_agent_file(content: &str) -> Option<AgentConfig> {
    // Strip UTF-8 BOM if present.
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);

    // Find two `---` delimiter lines.
    let lines: Vec<&str> = content.lines().collect();
    let mut delimiters: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "---" {
            delimiters.push(i);
            if delimiters.len() == 2 {
                break;
            }
        }
    }

    if delimiters.len() < 2 {
        return None;
    }

    let yaml_str: String = lines[delimiters[0] + 1..delimiters[1]].join("\n");
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(&yaml_str).ok()?;

    let meta = yaml_value.get("meta")?;
    let name = meta.get("name")?.as_str()?.to_string();
    let description = meta
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let tools = yaml_value
        .get("tools")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let body_start = delimiters[1] + 1;
    let instruction = lines[body_start..].join("\n").trim().to_string();

    Some(AgentConfig {
        name,
        description,
        tools,
        instruction,
    })
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
