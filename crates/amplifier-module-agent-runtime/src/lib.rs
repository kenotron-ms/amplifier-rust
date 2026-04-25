//! Agent runtime — AgentConfig, AgentRegistry, and bundle loading.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ModelRole
// ---------------------------------------------------------------------------

/// A single role name or an ordered fallback chain of role names.
///
/// `model_role:` in agent frontmatter may be a string or a list of strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ModelRole {
    /// A single role name (e.g. `"fast"`).
    Single(String),
    /// Ordered fallback chain tried left-to-right (e.g. `["reasoning", "general"]`).
    Chain(Vec<String>),
}

// ---------------------------------------------------------------------------
// ResolvedProvider
// ---------------------------------------------------------------------------

/// A `(provider, model)` pair plus opaque pass-through config, produced by
/// the routing resolver and stored on `AgentConfig::provider_preferences`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedProvider {
    /// Provider short-name (e.g. `"anthropic"`).
    pub provider: String,
    /// Concrete model name (e.g. `"claude-haiku-4"`).
    pub model: String,
    /// Opaque per-provider config (e.g. `{ reasoning_effort: "high" }`).
    #[serde(default)]
    pub config: serde_json::Value,
}

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
    /// Optional declared model role(s) — single name or fallback chain.
    /// Resolved by the routing hook at session start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_role: Option<ModelRole>,
    /// Resolved `(provider, model, config)` set by the routing hook.
    /// `None` until SessionStart fires and resolution succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_preferences: Option<Vec<ResolvedProvider>>,
}

// ---------------------------------------------------------------------------
// AgentRegistry
// ---------------------------------------------------------------------------

/// In-memory registry that maps agent names to their [`AgentConfig`].
#[derive(Clone)]
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

    /// Set the resolved provider preferences on an existing agent.
    ///
    /// No-op if no agent with `name` is registered.
    pub fn set_provider_preferences(&mut self, name: &str, prefs: Vec<ResolvedProvider>) {
        if let Some(cfg) = self.agents.get_mut(name) {
            cfg.provider_preferences = Some(prefs);
        }
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
            if let Some(config) = parse_agent_content(&content) {
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

/// Parse YAML frontmatter from an agent bundle `.md` file content into an [`AgentConfig`].
///
/// Returns `None` if the content cannot be parsed (missing frontmatter delimiters,
/// missing `meta.name`, or YAML parse error).
///
/// This is useful for loading agents from embedded `include_str!()` content as well
/// as from files read at runtime.
pub fn parse_agent_content(content: &str) -> Option<AgentConfig> {
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

    let model_role = yaml_value
        .get("model_role")
        .and_then(|v| serde_yaml::from_value::<ModelRole>(v.clone()).ok());

    Some(AgentConfig {
        name,
        description,
        tools,
        instruction,
        model_role,
        provider_preferences: None,
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
            model_role: None,
            provider_preferences: None,
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
                model_role: None,
                provider_preferences: None,
            });
        }
        let names: Vec<&str> = registry.list().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mango", "zebra"]);
    }

    #[test]
    fn agent_config_supports_model_role_single_and_chain() {
        let single = AgentConfig {
            name: "explorer".to_string(),
            description: String::new(),
            tools: vec![],
            instruction: String::new(),
            model_role: Some(ModelRole::Single("fast".to_string())),
            provider_preferences: None,
        };
        assert!(matches!(single.model_role, Some(ModelRole::Single(ref s)) if s == "fast"));

        let chain = AgentConfig {
            name: "zen-architect".to_string(),
            description: String::new(),
            tools: vec![],
            instruction: String::new(),
            model_role: Some(ModelRole::Chain(vec![
                "reasoning".to_string(),
                "general".to_string(),
            ])),
            provider_preferences: None,
        };
        assert!(matches!(&chain.model_role, Some(ModelRole::Chain(v)) if v.len() == 2));
    }

    #[test]
    fn parse_agent_content_reads_model_role_string() {
        let md = "---\nmeta:\n  name: explorer\n  description: x\nmodel_role: fast\n---\nbody";
        let cfg = parse_agent_content(md).expect("must parse");
        assert!(matches!(cfg.model_role, Some(ModelRole::Single(ref s)) if s == "fast"));
    }

    #[test]
    fn parse_agent_content_reads_model_role_list() {
        let md = "---\nmeta:\n  name: zen\n  description: x\nmodel_role:\n  - reasoning\n  - general\n---\nbody";
        let cfg = parse_agent_content(md).expect("must parse");
        assert!(matches!(&cfg.model_role, Some(ModelRole::Chain(v)) if v == &vec!["reasoning".to_string(), "general".to_string()]));
    }

    #[test]
    fn registry_set_provider_preferences_updates_existing_agent() {
        let mut registry = AgentRegistry::new();
        registry.register(AgentConfig {
            name: "a".to_string(),
            description: String::new(),
            tools: vec![],
            instruction: String::new(),
            model_role: Some(ModelRole::Single("fast".to_string())),
            provider_preferences: None,
        });
        let prefs = vec![ResolvedProvider {
            provider: "anthropic".to_string(),
            model: "claude-haiku-3".to_string(),
            config: serde_json::Value::Null,
        }];
        registry.set_provider_preferences("a", prefs.clone());
        assert_eq!(
            registry.get("a").unwrap().provider_preferences.as_ref().unwrap()[0].model,
            "claude-haiku-3"
        );
    }
}
