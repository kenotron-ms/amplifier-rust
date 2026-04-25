use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Arc;

use amplifier_core::traits::Provider;
pub use amplifier_module_agent_runtime::ResolvedProvider;
use globset::{Glob, GlobMatcher};
use regex::Regex;

use crate::matrix::{Candidate, RolesMap};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Map from provider name (with or without "provider-" prefix) to provider instance.
///
/// Keys may be either `"anthropic"` or `"provider-anthropic"`.
pub type ProviderMap = HashMap<String, Arc<dyn Provider>>;

// ---------------------------------------------------------------------------
// VersionPart / VersionKey
// ---------------------------------------------------------------------------

/// A single segment of a parsed version string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum VersionPart {
    /// A text run (letters, punctuation); compared lexicographically.
    Text(String),
    /// A digit run; compared numerically.
    Number(i64),
}

/// Comparable sort key for a model version name.
///
/// Parts are compared first (lexicographic/numeric by segment type), then
/// `neg_len` as a tiebreak — shorter original names sort higher, so clean
/// aliases (`gpt-5.4`) beat dated snapshots (`gpt-5.4-2026-03-05`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionKey {
    pub parts: Vec<VersionPart>,
    /// Negative of the original name length; higher value = shorter name.
    pub neg_len: i64,
}

// ---------------------------------------------------------------------------
// version_sort_key
// ---------------------------------------------------------------------------

/// Compute a `VersionKey` for `name` suitable for **descending** sort
/// (highest version first).
///
/// Algorithm:
/// 1. Strip trailing date suffix `-YYYYMMDD` or `-YYYY-MM-DD`.
/// 2. Split the stripped string on digit runs; each digit run becomes
///    `Number(n)`, each text run becomes `Text(s)`.
/// 3. Append `neg_len = -(original_name.len())` as tiebreak so the shorter
///    (un-dated) alias sorts above the dated snapshot when parts are equal.
pub fn version_sort_key(name: &str) -> VersionKey {
    // 1. Strip trailing date suffix
    let date_re = Regex::new(r"-(?:\d{4}-\d{2}-\d{2}|\d{8})$").unwrap();
    let stripped = date_re.replace(name, "");

    // 2. Split on digit runs
    let digit_re = Regex::new(r"\d+").unwrap();
    let mut parts: Vec<VersionPart> = Vec::new();
    let mut last_end: usize = 0;

    for m in digit_re.find_iter(stripped.as_ref()) {
        if m.start() > last_end {
            let text = &stripped[last_end..m.start()];
            if !text.is_empty() {
                parts.push(VersionPart::Text(text.to_string()));
            }
        }
        let n: i64 = m.as_str().parse().unwrap_or(0);
        parts.push(VersionPart::Number(n));
        last_end = m.end();
    }
    if last_end < stripped.len() {
        let text = &stripped[last_end..];
        if !text.is_empty() {
            parts.push(VersionPart::Text(text.to_string()));
        }
    }

    // 3. Tiebreak: shorter original name = better
    let neg_len = -(name.len() as i64);

    VersionKey { parts, neg_len }
}

// ---------------------------------------------------------------------------
// find_provider_by_type
// ---------------------------------------------------------------------------

/// Look up a provider by name, trying three forms in order:
/// 1. `name` (exact match)
/// 2. `name` with the `"provider-"` prefix stripped
/// 3. `name` with `"provider-"` prepended
pub fn find_provider_by_type<'a>(
    providers: &'a ProviderMap,
    name: &str,
) -> Option<&'a Arc<dyn Provider>> {
    // 1. Exact
    if let Some(p) = providers.get(name) {
        return Some(p);
    }
    // 2. Strip "provider-" prefix
    let stripped = name.trim_start_matches("provider-");
    if stripped != name {
        if let Some(p) = providers.get(stripped) {
            return Some(p);
        }
    }
    // 3. Add "provider-" prefix
    let prefixed = format!("provider-{}", name);
    if let Some(p) = providers.get(&prefixed) {
        return Some(p);
    }
    None
}

// ---------------------------------------------------------------------------
// is_glob
// ---------------------------------------------------------------------------

/// Returns `true` if the string contains any glob metacharacter (`*`, `?`, `[`).
pub fn is_glob(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

// ---------------------------------------------------------------------------
// resolve_glob
// ---------------------------------------------------------------------------

/// Resolve a glob pattern against a provider's model list.
///
/// - Compiles the glob; returns `None` on invalid pattern.
/// - Calls `provider.list_models()`; swallows ALL errors and returns `None`.
/// - Filters matching model IDs, sorts descending by `version_sort_key`,
///   and returns the highest-ranked match.
pub async fn resolve_glob(pattern: &str, provider: &Arc<dyn Provider>) -> Option<String> {
    let glob = Glob::new(pattern).ok()?;
    let matcher: GlobMatcher = glob.compile_matcher();

    let models = provider.list_models().await.ok()?;

    let mut matching: Vec<String> = models
        .into_iter()
        .filter(|m| matcher.is_match(&m.id))
        .map(|m| m.id)
        .collect();

    if matching.is_empty() {
        return None;
    }

    // Sort descending: highest version first
    matching.sort_by_key(|id| Reverse(version_sort_key(id)));

    matching.into_iter().next()
}

// ---------------------------------------------------------------------------
// resolve_model_role
// ---------------------------------------------------------------------------

/// Resolve a chain of model roles to a concrete provider+model pair.
///
/// Iterates `roles` (outer loop), then each role's candidates (inner loop).
/// Returns a single-element `Vec<ResolvedProvider>` for the first fully
/// resolvable candidate, or an empty `Vec` if nothing resolves.
///
/// Skip rules:
/// - Role not present in `matrix_roles` → skip role.
/// - Candidate is the `"base"` sentinel string → skip candidate.
/// - Provider not found via `find_provider_by_type` → skip candidate.
/// - Candidate model is a glob pattern and `resolve_glob` returns `None` → skip candidate.
pub async fn resolve_model_role(
    roles: &[String],
    matrix_roles: &RolesMap,
    providers: &ProviderMap,
) -> Vec<ResolvedProvider> {
    for role in roles {
        let role_config = match matrix_roles.get(role) {
            Some(r) => r,
            None => continue,
        };

        for candidate_value in &role_config.candidates {
            // Skip "base" sentinel
            if candidate_value.as_str() == Some("base") {
                continue;
            }

            // Try to deserialize as Candidate
            let candidate: Candidate = match serde_json::from_value(candidate_value.clone()) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Find provider
            let provider = match find_provider_by_type(providers, &candidate.provider) {
                Some(p) => p,
                None => continue,
            };

            // Resolve model (exact or via glob)
            let resolved_model = if is_glob(&candidate.model) {
                match resolve_glob(&candidate.model, provider).await {
                    Some(m) => m,
                    None => continue,
                }
            } else {
                candidate.model.clone()
            };

            return vec![ResolvedProvider {
                provider: candidate.provider.clone(),
                model: resolved_model,
                config: candidate.config.clone(),
            }];
        }
    }

    vec![]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::pin::Pin;

    use amplifier_core::{
        errors::ProviderError,
        messages::{ChatResponse, ToolCall},
        models::{ModelInfo, ProviderInfo},
        traits::Provider,
    };
    use serde_json::json;

    use crate::matrix::{RoleConfig, RolesMap};

    // -----------------------------------------------------------------------
    // StubProvider
    // -----------------------------------------------------------------------

    struct StubProvider {
        name: String,
        models: Vec<String>,
        fail: bool,
    }

    impl Provider for StubProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn get_info(&self) -> ProviderInfo {
            ProviderInfo {
                id: self.name.clone(),
                display_name: self.name.clone(),
                credential_env_vars: vec![],
                capabilities: vec![],
                defaults: Default::default(),
                config_fields: vec![],
            }
        }

        fn list_models(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>, ProviderError>> + Send + '_>>
        {
            let fail = self.fail;
            let models: Vec<String> = self.models.clone();
            Box::pin(async move {
                if fail {
                    return Err(ProviderError::Other {
                        message: "stub error".to_string(),
                        provider: None,
                        model: None,
                        retry_after: None,
                        status_code: None,
                        retryable: false,
                        delay_multiplier: None,
                    });
                }
                Ok(models
                    .into_iter()
                    .map(|id| ModelInfo {
                        id: id.clone(),
                        display_name: id,
                        context_window: 1024,
                        max_output_tokens: 1024,
                        capabilities: vec![],
                        defaults: Default::default(),
                    })
                    .collect())
            })
        }

        fn complete(
            &self,
            _request: amplifier_core::messages::ChatRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<amplifier_core::messages::ChatResponse, ProviderError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                Err(ProviderError::Other {
                    message: "not implemented in stub".to_string(),
                    provider: None,
                    model: None,
                    retry_after: None,
                    status_code: None,
                    retryable: false,
                    delay_multiplier: None,
                })
            })
        }

        fn parse_tool_calls(&self, _response: &ChatResponse) -> Vec<ToolCall> {
            vec![]
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_providers(entries: Vec<(&str, StubProvider)>) -> ProviderMap {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), Arc::new(v) as Arc<dyn Provider>))
            .collect()
    }

    fn make_roles(entries: Vec<(&str, Vec<serde_json::Value>)>) -> RolesMap {
        entries
            .into_iter()
            .map(|(name, candidates)| {
                (
                    name.to_string(),
                    RoleConfig {
                        description: format!("{} role", name),
                        candidates,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>()
    }

    // -----------------------------------------------------------------------
    // version_sort_key tests
    // -----------------------------------------------------------------------

    #[test]
    fn version_key_numeric_higher_wins() {
        let k10 = version_sort_key("claude-opus-4-10");
        let k7 = version_sort_key("claude-opus-4-7");
        assert!(
            k10 > k7,
            "claude-opus-4-10 should sort higher than claude-opus-4-7"
        );
    }

    #[test]
    fn version_key_alias_beats_snapshot() {
        let kalias = version_sort_key("gpt-5.4");
        let ksnap = version_sort_key("gpt-5.4-2026-03-05");
        assert!(
            kalias > ksnap,
            "gpt-5.4 should sort higher than gpt-5.4-2026-03-05"
        );
    }

    #[test]
    fn version_key_clean_version_above_dated() {
        let kclean = version_sort_key("claude-opus-4-7");
        let kdated = version_sort_key("claude-opus-4-20250514");
        assert!(
            kclean > kdated,
            "claude-opus-4-7 should sort higher than claude-opus-4-20250514"
        );
    }

    #[test]
    fn version_key_strips_compact_date_suffix() {
        let kshort = version_sort_key("foo-bar");
        let klong = version_sort_key("foo-bar-20240101");
        assert!(
            kshort > klong,
            "foo-bar should sort higher than foo-bar-20240101"
        );
    }

    // -----------------------------------------------------------------------
    // find_provider_by_type tests
    // -----------------------------------------------------------------------

    #[test]
    fn find_by_short_name() {
        let providers = make_providers(vec![(
            "anthropic",
            StubProvider {
                name: "anthropic".into(),
                models: vec![],
                fail: false,
            },
        )]);
        assert!(
            find_provider_by_type(&providers, "anthropic").is_some(),
            "exact 'anthropic' key should be found"
        );
    }

    #[test]
    fn find_by_provider_prefix_lookup() {
        // "provider-anthropic" lookup should strip prefix and hit "anthropic"
        let providers = make_providers(vec![(
            "anthropic",
            StubProvider {
                name: "anthropic".into(),
                models: vec![],
                fail: false,
            },
        )]);
        assert!(
            find_provider_by_type(&providers, "provider-anthropic").is_some(),
            "'provider-anthropic' should resolve to 'anthropic' entry"
        );
    }

    #[test]
    fn find_with_provider_prefix_in_map() {
        // "foo" lookup should add prefix and hit "provider-foo" map entry
        let providers = make_providers(vec![(
            "provider-foo",
            StubProvider {
                name: "provider-foo".into(),
                models: vec![],
                fail: false,
            },
        )]);
        assert!(
            find_provider_by_type(&providers, "foo").is_some(),
            "'foo' should resolve to 'provider-foo' entry"
        );
    }

    #[test]
    fn find_returns_none_when_missing() {
        let providers = make_providers(vec![(
            "anthropic",
            StubProvider {
                name: "anthropic".into(),
                models: vec![],
                fail: false,
            },
        )]);
        assert!(
            find_provider_by_type(&providers, "openai").is_none(),
            "openai should not be found in anthropic-only map"
        );
    }

    // -----------------------------------------------------------------------
    // resolve_glob tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_glob_picks_highest_version() {
        let provider: Arc<dyn Provider> = Arc::new(StubProvider {
            name: "anthropic".into(),
            models: vec![
                "claude-opus-4-7".to_string(),
                "claude-opus-4-10".to_string(),
                "claude-opus-4-20250514".to_string(),
            ],
            fail: false,
        });
        let result = resolve_glob("claude-opus-4-*", &provider).await;
        assert_eq!(result, Some("claude-opus-4-10".to_string()));
    }

    #[tokio::test]
    async fn resolve_glob_returns_none_when_no_match() {
        let provider: Arc<dyn Provider> = Arc::new(StubProvider {
            name: "anthropic".into(),
            models: vec!["claude-haiku-4".to_string()],
            fail: false,
        });
        let result = resolve_glob("gpt-*", &provider).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn resolve_glob_swallows_provider_error() {
        let provider: Arc<dyn Provider> = Arc::new(StubProvider {
            name: "anthropic".into(),
            models: vec![],
            fail: true,
        });
        let result = resolve_glob("claude-*", &provider).await;
        assert_eq!(result, None);
    }

    // -----------------------------------------------------------------------
    // resolve_model_role tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_model_role_exact_name_passes_through() {
        let providers = make_providers(vec![(
            "anthropic",
            StubProvider {
                name: "anthropic".into(),
                models: vec![],
                fail: false,
            },
        )]);
        let roles_map = make_roles(vec![(
            "general",
            vec![json!({"provider": "anthropic", "model": "claude-opus-4"})],
        )]);

        let result = resolve_model_role(&["general".to_string()], &roles_map, &providers).await;

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].provider, "anthropic");
        assert_eq!(result[0].model, "claude-opus-4");
    }

    #[tokio::test]
    async fn resolve_model_role_glob_picks_highest() {
        let providers = make_providers(vec![(
            "anthropic",
            StubProvider {
                name: "anthropic".into(),
                models: vec![
                    "claude-haiku-4".to_string(),
                    "claude-haiku-3".to_string(),
                    "claude-haiku-2-20240101".to_string(),
                ],
                fail: false,
            },
        )]);
        let roles_map = make_roles(vec![(
            "fast",
            vec![json!({"provider": "anthropic", "model": "claude-haiku-*"})],
        )]);

        let result = resolve_model_role(&["fast".to_string()], &roles_map, &providers).await;

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].model, "claude-haiku-4");
    }

    #[tokio::test]
    async fn resolve_model_role_falls_through_to_chain_member() {
        let providers = make_providers(vec![(
            "anthropic",
            StubProvider {
                name: "anthropic".into(),
                models: vec![],
                fail: false,
            },
        )]);
        // role_a: provider "openai" not in map → skip
        // role_b: provider "anthropic" found → resolves
        let roles_map = make_roles(vec![
            (
                "role_a",
                vec![json!({"provider": "openai", "model": "gpt-4"})],
            ),
            (
                "role_b",
                vec![json!({"provider": "anthropic", "model": "claude-opus-4"})],
            ),
        ]);

        let result = resolve_model_role(
            &["role_a".to_string(), "role_b".to_string()],
            &roles_map,
            &providers,
        )
        .await;

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].provider, "anthropic");
        assert_eq!(result[0].model, "claude-opus-4");
    }

    #[tokio::test]
    async fn resolve_model_role_returns_empty_when_unresolvable() {
        let providers = make_providers(vec![(
            "anthropic",
            StubProvider {
                name: "anthropic".into(),
                models: vec![],
                fail: false,
            },
        )]);
        // "base" sentinel is skipped; "openai" provider not found
        let roles_map = make_roles(vec![(
            "general",
            vec![
                json!("base"),
                json!({"provider": "openai", "model": "gpt-4"}),
            ],
        )]);

        let result = resolve_model_role(&["general".to_string()], &roles_map, &providers).await;

        assert!(
            result.is_empty(),
            "should return empty Vec when unresolvable"
        );
    }
}
