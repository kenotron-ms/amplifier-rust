use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single resolved model candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub config: serde_json::Value,
}

/// Configuration for a single model role, as stored in a matrix / override file.
///
/// `candidates` holds raw JSON values because during composition the special
/// sentinel string `"base"` is allowed in override files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleConfig {
    pub description: String,
    pub candidates: Vec<serde_json::Value>,
}

/// Map of role-name → role configuration.
///
/// `BTreeMap` is intentional: deterministic iteration order is required for
/// tests and for generating stable prompt output.
pub type RolesMap = BTreeMap<String, RoleConfig>;

/// Top-level structure of a matrix YAML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixConfig {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub updated: String,
    pub roles: RolesMap,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a `RolesMap` from either a full matrix file or an override file.
///
/// Rules:
/// - `is_override = false` (matrix file): both `general` and `fast` roles must exist.
/// - Every role: non-empty `description` and non-empty `candidates`.
/// - `is_override = false`: literal `"base"` candidates are rejected.
/// - `is_override = true`: `"base"` is allowed; `general`/`fast` not required.
pub fn validate_matrix(roles: &RolesMap, is_override: bool) -> anyhow::Result<()> {
    if !is_override {
        if !roles.contains_key("general") {
            anyhow::bail!("matrix must contain a 'general' role");
        }
        if !roles.contains_key("fast") {
            anyhow::bail!("matrix must contain a 'fast' role");
        }
    }

    for (name, role) in roles {
        if role.description.is_empty() {
            anyhow::bail!("role '{}' has an empty description", name);
        }
        if role.candidates.is_empty() {
            anyhow::bail!("role '{}' has empty candidates", name);
        }
        if !is_override {
            for candidate in &role.candidates {
                if candidate.as_str() == Some("base") {
                    anyhow::bail!(
                        "role '{}' contains a 'base' sentinel candidate, which is not allowed in a matrix file",
                        name
                    );
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

/// Load a `MatrixConfig` by searching for `<name>.yaml` in each of `dirs` in order.
///
/// The first matching file wins.  Returns an error if no file is found in any of
/// the provided directories, or if the file fails to parse.
pub fn load_matrix_from_dirs(name: &str, dirs: &[&Path]) -> anyhow::Result<MatrixConfig> {
    for dir in dirs {
        let candidate = dir.join(format!("{name}.yaml"));
        if candidate.is_file() {
            let content = std::fs::read_to_string(&candidate)
                .map_err(|e| anyhow::anyhow!("failed to read {}: {}", candidate.display(), e))?;
            let config: MatrixConfig = serde_yaml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", candidate.display(), e))?;
            return Ok(config);
        }
    }
    anyhow::bail!("matrix '{}' not found in any search directory", name)
}

/// Return the default 3-level search path for matrix YAML files:
///
/// 1. `<cwd>/.amplifier/routing`
/// 2. `~/.amplifier/routing` (if the `HOME` environment variable is set)
/// 3. `<crate>/routing` (bundled, always included)
///
/// Non-existent directories are included; they simply yield no match in
/// [`load_matrix_from_dirs`].
pub fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Level 1: cwd/.amplifier/routing
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".amplifier").join("routing"));
    }

    // Level 2: ~/.amplifier/routing
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".amplifier").join("routing"));
    }

    // Level 3: bundled crate/routing (always present in the binary)
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("routing"));

    dirs
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a raw JSON value from a `candidates` list into a `Candidate`.
///
/// Returns `None` if the value is the `"base"` sentinel string, or if
/// deserialisation fails.
pub fn candidate_from_value(v: &serde_json::Value) -> Option<Candidate> {
    if v.as_str() == Some("base") {
        return None;
    }
    serde_json::from_value(v.clone()).ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_role(description: &str, candidates: Vec<serde_json::Value>) -> RoleConfig {
        RoleConfig {
            description: description.to_string(),
            candidates,
        }
    }

    fn minimal_candidate() -> serde_json::Value {
        json!({"provider": "openai", "model": "gpt-4o"})
    }

    // 1. Happy path: a minimal valid matrix (general + fast) should pass.
    #[test]
    fn validate_passes_for_minimal_valid_matrix() {
        let mut roles = RolesMap::new();
        roles.insert(
            "general".into(),
            make_role("General purpose", vec![minimal_candidate()]),
        );
        roles.insert(
            "fast".into(),
            make_role("Fast responses", vec![minimal_candidate()]),
        );
        assert!(validate_matrix(&roles, false).is_ok());
    }

    // 2. Missing 'general' role must be rejected with a message mentioning "general".
    #[test]
    fn validate_rejects_missing_general() {
        let mut roles = RolesMap::new();
        roles.insert(
            "fast".into(),
            make_role("Fast responses", vec![minimal_candidate()]),
        );
        let err = validate_matrix(&roles, false).unwrap_err();
        assert!(
            err.to_string().contains("general"),
            "expected 'general' in error: {err}"
        );
    }

    // 3. Missing 'fast' role must be rejected with a message mentioning "fast".
    #[test]
    fn validate_rejects_missing_fast() {
        let mut roles = RolesMap::new();
        roles.insert(
            "general".into(),
            make_role("General purpose", vec![minimal_candidate()]),
        );
        let err = validate_matrix(&roles, false).unwrap_err();
        assert!(
            err.to_string().contains("fast"),
            "expected 'fast' in error: {err}"
        );
    }

    // 4. A "base" sentinel in a non-override matrix must be rejected; error mentions "base".
    #[test]
    fn validate_rejects_base_in_matrix_file() {
        let mut roles = RolesMap::new();
        roles.insert(
            "general".into(),
            make_role("General purpose", vec![json!("base")]),
        );
        roles.insert(
            "fast".into(),
            make_role("Fast responses", vec![minimal_candidate()]),
        );
        let err = validate_matrix(&roles, false).unwrap_err();
        assert!(
            err.to_string().contains("base"),
            "expected 'base' in error: {err}"
        );
    }

    // 5. A "base" sentinel in an override file must be allowed.
    #[test]
    fn validate_allows_base_in_override() {
        let mut roles = RolesMap::new();
        roles.insert(
            "general".into(),
            make_role("General purpose", vec![json!("base")]),
        );
        assert!(validate_matrix(&roles, true).is_ok());
    }

    // 6. Empty candidates Vec must be rejected; error mentions "empty candidates".
    #[test]
    fn validate_rejects_empty_candidates() {
        let mut roles = RolesMap::new();
        roles.insert("general".into(), make_role("General purpose", vec![]));
        roles.insert(
            "fast".into(),
            make_role("Fast responses", vec![minimal_candidate()]),
        );
        let err = validate_matrix(&roles, false).unwrap_err();
        assert!(
            err.to_string().contains("empty candidates"),
            "expected 'empty candidates' in error: {err}"
        );
    }

    // 7. candidate_from_value: "base" → None; valid object → Some(Candidate).
    #[test]
    fn candidate_from_value_skips_base_sentinel() {
        assert_eq!(candidate_from_value(&json!("base")), None);

        let v = json!({"provider": "anthropic", "model": "claude-3-5-sonnet-20241022"});
        let result = candidate_from_value(&v);
        assert!(result.is_some());
        let c = result.unwrap();
        assert_eq!(c.provider, "anthropic");
        assert_eq!(c.model, "claude-3-5-sonnet-20241022");
    }

    // 8. loader_finds_bundled_matrix — loads balanced.yaml from bundled routing/ dir.
    //    This test will fail until Task 7 writes balanced.yaml — that is intentional.
    #[test]
    fn loader_finds_bundled_matrix() {
        use std::path::PathBuf;
        let bundled_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("routing");
        let cfg = load_matrix_from_dirs("balanced", &[bundled_dir.as_path()])
            .expect("balanced.yaml should exist in bundled routing/ dir");
        assert_eq!(cfg.name, "balanced");
        assert!(validate_matrix(&cfg.roles, false).is_ok());
    }

    // 9. loader_returns_err_when_not_found — error message contains the matrix name.
    #[test]
    fn loader_returns_err_when_not_found() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dirs = [tmp.path()];
        let err = load_matrix_from_dirs("does-not-exist", &dirs)
            .expect_err("should error when matrix file not found");
        assert!(
            err.to_string().contains("does-not-exist"),
            "expected 'does-not-exist' in error: {err}"
        );
    }

    // 11. all_bundled_matrices_validate — all 7 bundled matrix YAML files exist and validate.
    #[test]
    fn all_bundled_matrices_validate() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("routing");
        for name in [
            "balanced",
            "quality",
            "economy",
            "anthropic",
            "openai",
            "gemini",
            "copilot",
        ] {
            let cfg = load_matrix_from_dirs(name, &[dir.as_path()])
                .unwrap_or_else(|e| panic!("{name}.yaml: {e}"));
            validate_matrix(&cfg.roles, false).unwrap_or_else(|e| panic!("{name}.yaml: {e}"));
        }
    }

    // 10. loader_first_match_wins — first dir's content wins over second dir.
    #[test]
    fn loader_first_match_wins() {
        use std::io::Write;

        let tmp1 = tempfile::tempdir().expect("tempdir 1");
        let tmp2 = tempfile::tempdir().expect("tempdir 2");

        let yaml_first = r#"
name: first
description: From first dir
roles:
  general:
    description: General role
    candidates:
      - provider: openai
        model: gpt-4o
  fast:
    description: Fast role
    candidates:
      - provider: openai
        model: gpt-4o-mini
"#;

        let yaml_second = r#"
name: second
description: From second dir
roles:
  general:
    description: General role
    candidates:
      - provider: anthropic
        model: claude-3-5-haiku-20241022
  fast:
    description: Fast role
    candidates:
      - provider: anthropic
        model: claude-3-5-haiku-20241022
"#;

        let mut f1 = std::fs::File::create(tmp1.path().join("test.yaml")).expect("create f1");
        f1.write_all(yaml_first.as_bytes()).expect("write f1");

        let mut f2 = std::fs::File::create(tmp2.path().join("test.yaml")).expect("create f2");
        f2.write_all(yaml_second.as_bytes()).expect("write f2");

        let dirs = [tmp1.path(), tmp2.path()];
        let cfg = load_matrix_from_dirs("test", &dirs).expect("should find test.yaml");
        assert_eq!(cfg.name, "first", "first dir should win");
    }
}
