//! Model-role routing hook — Rust port of `amplifier-module-hooks-routing`.

pub mod composer;
pub mod matrix;
pub mod resolver;

pub use amplifier_module_agent_runtime::ResolvedProvider;
pub use matrix::{Candidate, MatrixConfig, RoleConfig, RolesMap};
pub use resolver::{resolve_model_role, ProviderMap};

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use amplifier_module_agent_runtime::{AgentRegistry, ModelRole};

use crate::matrix::default_search_dirs;

// ---------------------------------------------------------------------------
// RoutingConfig
// ---------------------------------------------------------------------------

/// Configuration for the model-role routing hook.
#[derive(Debug, Clone)]
pub struct RoutingConfig {
    /// Matrix name (file basename without `.yaml`).  Defaults to `"balanced"`.
    pub default_matrix: String,
    /// Optional override JSON in the same shape as a matrix file.
    pub overrides: Option<serde_json::Value>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            default_matrix: "balanced".into(),
            overrides: None,
        }
    }
}

// ---------------------------------------------------------------------------
// HookEvent
// ---------------------------------------------------------------------------

/// Events that hooks can subscribe to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// Fired once on session start; hooks resolve model roles for all agents.
    SessionStart,
    /// Fired before each provider request; hooks may inject context.
    ProviderRequest,
}

// ---------------------------------------------------------------------------
// HookContext
// ---------------------------------------------------------------------------

/// Context passed to each hook handler.
///
/// Intentionally empty — reserved for future expansion (e.g., session ID,
/// agent name, request metadata).
#[derive(Debug, Clone, Default)]
pub struct HookContext;

// ---------------------------------------------------------------------------
// HookResult
// ---------------------------------------------------------------------------

/// Return value from a hook handler.
#[derive(Debug, Clone, PartialEq)]
pub enum HookResult {
    /// Continue with the normal flow — no additional action needed.
    Continue,
    /// Inject additional text into the agent's conversation context.
    InjectContext(String),
}

// ---------------------------------------------------------------------------
// Hook trait
// ---------------------------------------------------------------------------

/// Trait implemented by all hooks that can be registered with [`HookRegistry`].
#[async_trait]
pub trait Hook: Send + Sync {
    /// Events this hook wants to receive.
    fn events(&self) -> &[HookEvent];

    /// Handle one event invocation.
    async fn handle(&self, ctx: &HookContext) -> HookResult;
}

// ---------------------------------------------------------------------------
// HookRegistry
// ---------------------------------------------------------------------------

/// Registry that holds hooks and dispatches events to them in priority order.
///
/// Lower priority number = earlier execution.
pub struct HookRegistry {
    /// Hooks sorted by ascending priority.
    hooks: Vec<(i32, Arc<dyn Hook>)>,
}

impl HookRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a hook at the given priority level (lower = earlier).
    pub fn register(&mut self, hook: Arc<dyn Hook>, priority: i32) {
        self.hooks.push((priority, hook));
        self.hooks.sort_by_key(|(p, _)| *p);
    }

    /// Emit an event to all subscribed hooks in priority order.
    ///
    /// Returns all results collected from hooks subscribed to `event`.
    pub async fn emit(&self, event: HookEvent, ctx: &HookContext) -> Vec<HookResult> {
        let mut results = Vec::new();
        for (_, hook) in &self.hooks {
            if hook.events().contains(&event) {
                let result = hook.handle(ctx).await;
                results.push(result);
            }
        }
        results
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SessionStartHook
// ---------------------------------------------------------------------------

/// Hook that resolves each agent's `model_role` to concrete provider
/// preferences on session start.
///
/// Algorithm (deadlock-safe):
/// 1. Clone the provider map snapshot (cheap Arc clones).
/// 2. Read-lock the agent registry to snapshot `(name, roles)` pairs; release.
/// 3. Await `resolve_model_role` for each agent with no lock held on registry.
/// 4. Write-lock the registry once to apply all resolved preferences.
pub struct SessionStartHook {
    matrix: MatrixConfig,
    agent_registry: Arc<RwLock<AgentRegistry>>,
    providers: Arc<RwLock<ProviderMap>>,
}

#[async_trait]
impl Hook for SessionStartHook {
    fn events(&self) -> &[HookEvent] {
        const EVENTS: &[HookEvent] = &[HookEvent::SessionStart];
        EVENTS
    }

    async fn handle(&self, _ctx: &HookContext) -> HookResult {
        // Step 1: snapshot the provider map (Arc clones only, lock released immediately).
        let providers_snapshot: ProviderMap = {
            let guard = self.providers.read().await;
            guard.clone()
        };

        // Step 2: snapshot (agent_name, roles) pairs; release read lock before any await.
        let snapshots: Vec<(String, Vec<String>)> = {
            let registry = self.agent_registry.read().await;
            registry
                .list()
                .iter()
                .filter_map(|cfg| {
                    cfg.model_role.as_ref().map(|role| {
                        let roles = match role {
                            ModelRole::Single(r) => vec![r.clone()],
                            ModelRole::Chain(rs) => rs.clone(),
                        };
                        (cfg.name.clone(), roles)
                    })
                })
                .collect()
        };
        // agent_registry read lock is released here.

        // Step 3: resolve each agent — no lock held on agent_registry during await.
        let mut updates: Vec<(String, Vec<ResolvedProvider>)> = Vec::new();
        for (agent_name, roles) in snapshots {
            let resolved =
                resolve_model_role(&roles, &self.matrix.roles, &providers_snapshot).await;
            if !resolved.is_empty() {
                updates.push((agent_name, resolved));
            }
        }

        // Step 4: apply all updates under a single write lock.
        {
            let mut registry = self.agent_registry.write().await;
            for (name, prefs) in updates {
                registry.set_provider_preferences(&name, prefs);
            }
        }

        HookResult::Continue
    }
}

// ---------------------------------------------------------------------------
// ProviderRequestHook
// ---------------------------------------------------------------------------

/// Hook that injects the active routing matrix name and role catalog into
/// the agent's context before each provider request.
pub struct ProviderRequestHook {
    matrix_name: String,
    roles: RolesMap,
}

#[async_trait]
impl Hook for ProviderRequestHook {
    fn events(&self) -> &[HookEvent] {
        const EVENTS: &[HookEvent] = &[HookEvent::ProviderRequest];
        EVENTS
    }

    async fn handle(&self, _ctx: &HookContext) -> HookResult {
        let mut buf = format!(
            "Active routing matrix: {}\n\nAvailable model roles (use model_role parameter when delegating):\n",
            self.matrix_name
        );
        // RolesMap = BTreeMap → deterministic alphabetical iteration order.
        for (role_name, role_config) in &self.roles {
            buf.push_str(&format!("  {} — {}\n", role_name, role_config.description));
        }
        HookResult::InjectContext(buf)
    }
}

// ---------------------------------------------------------------------------
// HooksRouting
// ---------------------------------------------------------------------------

/// Routing hook that loads a model-role matrix and resolves provider
/// preferences for agents.
#[allow(dead_code)]
pub struct HooksRouting {
    matrix: MatrixConfig,
    agent_registry: Arc<RwLock<AgentRegistry>>,
    providers: Arc<RwLock<ProviderMap>>,
}

impl HooksRouting {
    /// Create a new `HooksRouting` from a [`RoutingConfig`] and an
    /// `AgentRegistry` wrapped in a shared lock.
    ///
    /// Steps:
    /// 1. Build the default search path.
    /// 2. Load the matrix YAML identified by `config.default_matrix`.
    /// 3. If `config.overrides` is present, apply them via [`composer::compose_matrix`].
    ///    Otherwise, validate the loaded matrix strictly.
    /// 4. Return `Self` with an empty provider map.
    pub fn new(
        config: RoutingConfig,
        agent_registry: Arc<RwLock<AgentRegistry>>,
    ) -> anyhow::Result<Self> {
        let dirs = default_search_dirs();
        let dir_refs: Vec<&Path> = dirs.iter().map(|p| p.as_path()).collect();

        let mut loaded_matrix = matrix::load_matrix_from_dirs(&config.default_matrix, &dir_refs)?;

        if let Some(overrides) = &config.overrides {
            loaded_matrix.roles = composer::compose_matrix(&loaded_matrix.roles, overrides)?;
        } else {
            matrix::validate_matrix(&loaded_matrix.roles, false)?;
        }

        Ok(Self {
            matrix: loaded_matrix,
            agent_registry,
            providers: Arc::new(RwLock::new(ProviderMap::new())),
        })
    }

    /// Replace the provider map.  Called by the sandbox after providers register.
    pub async fn set_providers(&self, providers: ProviderMap) {
        let mut guard = self.providers.write().await;
        *guard = providers;
    }

    /// Return the name of the loaded matrix (e.g. `"balanced"`).
    pub fn matrix_name(&self) -> &str {
        &self.matrix.name
    }

    /// Look up a role by name in the loaded matrix.
    pub fn role(&self, name: &str) -> Option<&RoleConfig> {
        self.matrix.roles.get(name)
    }

    /// Return all role names defined in the loaded matrix.
    pub fn role_names(&self) -> Vec<String> {
        self.matrix.roles.keys().cloned().collect()
    }

    /// Register this routing module's hooks onto a [`HookRegistry`].
    ///
    /// Registers:
    /// - [`SessionStartHook`] at priority 5 — resolves model roles on session start.
    /// - [`ProviderRequestHook`] at priority 15 — injects matrix catalog into context.
    pub fn register_on(&self, registry: &mut HookRegistry) {
        let session_start = SessionStartHook {
            matrix: self.matrix.clone(),
            agent_registry: Arc::clone(&self.agent_registry),
            providers: Arc::clone(&self.providers),
        };
        registry.register(Arc::new(session_start), 5);

        let provider_request = ProviderRequestHook {
            matrix_name: self.matrix.name.clone(),
            roles: self.matrix.roles.clone(),
        };
        registry.register(Arc::new(provider_request), 15);
    }
}
