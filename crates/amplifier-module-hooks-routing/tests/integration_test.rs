//! Integration tests for HooksRouting.

use std::sync::Arc;
use tokio::sync::RwLock;

use amplifier_module_agent_runtime::AgentRegistry;
use amplifier_module_hooks_routing::{HooksRouting, RoutingConfig};

#[test]
fn new_loads_balanced_by_default() {
    let registry = Arc::new(RwLock::new(AgentRegistry::new()));
    let routing = HooksRouting::new(RoutingConfig::default(), registry).expect("should load");
    assert_eq!(routing.matrix_name(), "balanced");
    assert!(routing.role_names().iter().any(|s| s == "general"));
    assert!(routing.role_names().iter().any(|s| s == "fast"));
}

#[test]
fn new_applies_overrides() {
    let registry = Arc::new(RwLock::new(AgentRegistry::new()));
    let cfg = RoutingConfig {
        default_matrix: "balanced".into(),
        overrides: Some(serde_json::json!({
            "roles": {
                "fast": {
                    "candidates": [{"provider": "ollama", "model": "llama3.2"}]
                }
            }
        })),
    };
    let routing = HooksRouting::new(cfg, registry).expect("should load with overrides");
    let fast = routing.role("fast").expect("fast must exist");
    assert_eq!(fast.candidates.len(), 1);
    assert_eq!(fast.candidates[0]["provider"], "ollama");
}

#[test]
fn new_errors_on_unknown_matrix() {
    let registry = Arc::new(RwLock::new(AgentRegistry::new()));
    let cfg = RoutingConfig {
        default_matrix: "does-not-exist-anywhere".into(),
        overrides: None,
    };
    assert!(HooksRouting::new(cfg, registry).is_err());
}
