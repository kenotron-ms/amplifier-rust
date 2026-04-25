use serde_json::Value;

use crate::matrix::{validate_matrix, RoleConfig, RolesMap};

/// Compose a base [`RolesMap`] with a JSON overrides value.
///
/// The `overrides` value is expected to have a `"roles"` object at the top
/// level (same schema as a matrix file).  Each override role can include a
/// `"base"` sentinel string in its `candidates` array:
///
/// - **0 `"base"` tokens** — the override's candidate list fully replaces
///   the base role's candidates.
/// - **1 `"base"` token** — the base candidates are spliced in at that position.
/// - **2+ `"base"` tokens** — returns `Err`.
///
/// Roles absent from `overrides` are inherited from `base` unchanged.
///
/// After composition the result is validated with `validate_matrix(_, false)`
/// (strict matrix validation — no `"base"` sentinels allowed).
pub fn compose_matrix(base: &RolesMap, overrides: &Value) -> anyhow::Result<RolesMap> {
    // Empty / missing overrides → return base unchanged.
    let override_roles = match overrides.get("roles").and_then(|v| v.as_object()) {
        Some(obj) if !obj.is_empty() => obj,
        _ => return Ok(base.clone()),
    };

    let mut out: RolesMap = base.clone();

    for (role_name, override_role) in override_roles {
        // Get the override candidates array (default to empty slice).
        let override_candidates: Vec<Value> = override_role
            .get("candidates")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // Count "base" sentinel tokens.
        let base_count = override_candidates
            .iter()
            .filter(|v| v.as_str() == Some("base"))
            .count();

        if base_count > 1 {
            anyhow::bail!(
                "role '{}' has {} 'base' sentinel tokens; at most 1 is allowed",
                role_name,
                base_count
            );
        }

        // Read base candidates (default empty if role is missing in base).
        let base_candidates: Vec<Value> = base
            .get(role_name)
            .map(|rc| rc.candidates.clone())
            .unwrap_or_default();

        // Build the composed candidate list.
        let composed_candidates: Vec<Value> = if base_count == 0 {
            // Full replacement.
            override_candidates
        } else {
            // Splice: expand the single "base" sentinel with all base candidates.
            let mut result = Vec::new();
            for v in &override_candidates {
                if v.as_str() == Some("base") {
                    result.extend_from_slice(&base_candidates);
                } else {
                    result.push(v.clone());
                }
            }
            result
        };

        // Determine description: prefer override, else inherit from base, else empty.
        let description = override_role
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| base.get(role_name).map(|rc| rc.description.clone()))
            .unwrap_or_default();

        out.insert(
            role_name.clone(),
            RoleConfig {
                description,
                candidates: composed_candidates,
            },
        );
    }

    validate_matrix(&out, false)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    /// Build a 3-role base map: general, fast, reasoning.
    fn make_base() -> RolesMap {
        let mut m: RolesMap = BTreeMap::new();
        m.insert(
            "general".to_string(),
            RoleConfig {
                description: "General purpose".to_string(),
                candidates: vec![
                    json!({"provider": "anthropic", "model": "claude-3-5-sonnet-20241022"}),
                ],
            },
        );
        m.insert(
            "fast".to_string(),
            RoleConfig {
                description: "Fast responses".to_string(),
                candidates: vec![json!({"provider": "openai", "model": "gpt-4o-mini"})],
            },
        );
        m.insert(
            "reasoning".to_string(),
            RoleConfig {
                description: "Deep reasoning".to_string(),
                candidates: vec![
                    json!({"provider": "anthropic", "model": "claude-3-7-sonnet-thinking"}),
                ],
            },
        );
        m
    }

    /// Override `reasoning` with a single openai candidate (0 "base" tokens).
    /// Result: reasoning has exactly 1 candidate with provider == "openai".
    #[test]
    fn no_base_token_replaces_role_candidates() {
        let base = make_base();
        let overrides = json!({
            "roles": {
                "reasoning": {
                    "description": "Overridden reasoning",
                    "candidates": [{"provider": "openai", "model": "o1"}]
                }
            }
        });
        let result = compose_matrix(&base, &overrides).expect("should succeed");
        let reasoning = result.get("reasoning").expect("reasoning role present");
        assert_eq!(
            reasoning.candidates.len(),
            1,
            "should have exactly 1 candidate"
        );
        assert_eq!(
            reasoning.candidates[0]
                .get("provider")
                .and_then(|v| v.as_str()),
            Some("openai"),
            "provider should be openai"
        );
    }

    /// Override `reasoning` with [openai, "base", ollama] (1 "base" token).
    /// Result: len==3; cands[0]==openai, cands[1]==anthropic (from base), cands[2]==ollama.
    #[test]
    fn single_base_token_splices_base_candidates() {
        let base = make_base();
        let overrides = json!({
            "roles": {
                "reasoning": {
                    "description": "Spliced reasoning",
                    "candidates": [
                        {"provider": "openai", "model": "o1"},
                        "base",
                        {"provider": "ollama", "model": "llama3"}
                    ]
                }
            }
        });
        let result = compose_matrix(&base, &overrides).expect("should succeed");
        let reasoning = result.get("reasoning").expect("reasoning role present");
        assert_eq!(
            reasoning.candidates.len(),
            3,
            "should have exactly 3 candidates"
        );
        assert_eq!(
            reasoning.candidates[0]
                .get("provider")
                .and_then(|v| v.as_str()),
            Some("openai"),
            "cands[0] provider should be openai"
        );
        assert_eq!(
            reasoning.candidates[1]
                .get("provider")
                .and_then(|v| v.as_str()),
            Some("anthropic"),
            "cands[1] provider should be anthropic (spliced from base)"
        );
        assert_eq!(
            reasoning.candidates[2]
                .get("provider")
                .and_then(|v| v.as_str()),
            Some("ollama"),
            "cands[2] provider should be ollama"
        );
    }

    /// Override with ["base", x, "base"] — 2 "base" tokens — must return Err
    /// whose message contains "'base'".
    #[test]
    fn double_base_token_errors() {
        let base = make_base();
        let overrides = json!({
            "roles": {
                "reasoning": {
                    "description": "Bad override",
                    "candidates": [
                        "base",
                        {"provider": "openai", "model": "o1"},
                        "base"
                    ]
                }
            }
        });
        let err = compose_matrix(&base, &overrides).expect_err("should error with 2 'base' tokens");
        assert!(
            err.to_string().contains("'base'"),
            "error message should mention 'base', got: {err}"
        );
    }

    /// Override only `reasoning`; `general` and `fast` must be inherited
    /// unchanged (description and candidates preserved).
    #[test]
    fn untouched_roles_inherit_from_base() {
        let base = make_base();
        let overrides = json!({
            "roles": {
                "reasoning": {
                    "description": "Overridden reasoning",
                    "candidates": [{"provider": "openai", "model": "o1"}]
                }
            }
        });
        let result = compose_matrix(&base, &overrides).expect("should succeed");

        let general = result.get("general").expect("general role present");
        assert_eq!(general.description, "General purpose");
        assert_eq!(general.candidates.len(), 1);
        assert_eq!(
            general.candidates[0]
                .get("provider")
                .and_then(|v| v.as_str()),
            Some("anthropic"),
            "general provider unchanged"
        );

        let fast = result.get("fast").expect("fast role present");
        assert_eq!(fast.description, "Fast responses");
        assert_eq!(fast.candidates.len(), 1);
        assert_eq!(
            fast.candidates[0].get("provider").and_then(|v| v.as_str()),
            Some("openai"),
            "fast provider unchanged"
        );
    }

    /// Empty overrides object returns base unchanged (3 roles).
    #[test]
    fn empty_overrides_returns_base() {
        let base = make_base();
        let overrides = json!({});
        let result = compose_matrix(&base, &overrides).expect("should succeed");
        assert_eq!(result.len(), 3, "should have 3 roles");
        assert_eq!(result, base, "result should equal base");
    }
}
