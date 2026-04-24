//! Skills tool module — provides the `load_skill` tool for discovering and
//! dispatching Amplifier skills from the file system.

pub mod parser;

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Value};

use amplifier_core::errors::ToolError;
use amplifier_core::messages::ToolSpec;
use amplifier_core::models::ToolResult;
use amplifier_core::traits::Tool;

use amplifier_module_tool_task::{ContextDepth, ContextScope, SpawnRequest, SubagentRunner};

use crate::parser::{parse_skill_md, ParsedSkill, SkillContext};

// ---------------------------------------------------------------------------
// SkillEngine
// ---------------------------------------------------------------------------

/// Engine for discovering and dispatching Amplifier skills from the file system.
///
/// # Search paths (in priority order)
///
/// 1. `<vault_path>/skills/`
/// 2. `$HOME/.amplifier/skills/`
pub struct SkillEngine {
    search_paths: Vec<PathBuf>,
    runner: Option<Arc<dyn SubagentRunner>>,
}

impl SkillEngine {
    /// Create a new [`SkillEngine`] with search paths derived from `vault_path`.
    ///
    /// Automatically adds `<vault_path>/skills/` and `$HOME/.amplifier/skills/`
    /// to the search path list.
    pub fn new(vault_path: impl Into<PathBuf>) -> Self {
        let vault_path: PathBuf = vault_path.into();
        let mut search_paths = Vec::new();

        // 1. <vault_path>/skills/
        search_paths.push(vault_path.join("skills"));

        // 2. $HOME/.amplifier/skills/
        if let Some(home) = dirs_next_home() {
            search_paths.push(home.join(".amplifier").join("skills"));
        }

        Self {
            search_paths,
            runner: None,
        }
    }

    /// Attach a [`SubagentRunner`] (required for Fork skills).
    pub fn with_runner(mut self, runner: Arc<dyn SubagentRunner>) -> Self {
        self.runner = Some(runner);
        self
    }

    /// Discover all skills across all configured search paths.
    ///
    /// For each search path that is a directory, reads every sub-directory
    /// that contains a `SKILL.md` file and parses it.  Parse errors are
    /// silently ignored.
    fn discover_skills(&self) -> Vec<ParsedSkill> {
        let mut skills = Vec::new();

        for search_path in &self.search_paths {
            if !search_path.is_dir() {
                continue;
            }

            let entries = match std::fs::read_dir(search_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let skill_md_path = entry.path().join("SKILL.md");
                if !skill_md_path.is_file() {
                    continue;
                }

                let content = match std::fs::read_to_string(&skill_md_path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let mut parsed = match parse_skill_md(&content) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Populate directory from the entry path, not from the YAML.
                parsed.frontmatter.directory = entry.path();
                skills.push(parsed);
            }
        }

        skills
    }

    /// List all discovered skills.
    ///
    /// Returns a JSON array with `name`, `description`, `context`, and
    /// `user_invocable` fields for each skill.
    fn handle_list(&self) -> Value {
        let skills = self.discover_skills();
        let arr: Vec<Value> = skills
            .iter()
            .map(|s| {
                json!({
                    "name": s.frontmatter.name,
                    "description": s.frontmatter.description,
                    "context": format!("{:?}", s.frontmatter.context).to_lowercase(),
                    "user_invocable": s.frontmatter.user_invocable,
                })
            })
            .collect();
        json!(arr)
    }

    /// Search skills by keyword (case-insensitive match against name or description).
    ///
    /// Returns a JSON array with `name` and `description` fields.
    fn handle_search(&self, query: &str) -> Value {
        let skills = self.discover_skills();
        let q = query.to_lowercase();
        let arr: Vec<Value> = skills
            .iter()
            .filter(|s| {
                s.frontmatter.name.to_lowercase().contains(&q)
                    || s.frontmatter.description.to_lowercase().contains(&q)
            })
            .map(|s| {
                json!({
                    "name": s.frontmatter.name,
                    "description": s.frontmatter.description,
                })
            })
            .collect();
        json!(arr)
    }

    /// Return frontmatter + body preview (first 200 characters) for a named skill.
    fn handle_info(&self, skill_name: &str) -> anyhow::Result<Value> {
        let skills = self.discover_skills();
        let skill = skills
            .iter()
            .find(|s| s.frontmatter.name == skill_name)
            .ok_or_else(|| anyhow::anyhow!("skill not found: {skill_name}"))?;

        let body_preview: String = skill.body.chars().take(200).collect();

        Ok(json!({
            "name": skill.frontmatter.name,
            "description": skill.frontmatter.description,
            "context": format!("{:?}", skill.frontmatter.context).to_lowercase(),
            "user_invocable": skill.frontmatter.user_invocable,
            "body_preview": body_preview,
        }))
    }

    /// Load and dispatch a skill.
    ///
    /// - `Inject`: returns `{skill_name, context: "inject", body}` for the
    ///   orchestrator to inject into the current conversation.
    /// - `Fork`: spawns a sub-agent via the configured [`SubagentRunner`]
    ///   and returns `{skill_name, context: "fork", result}`.
    async fn handle_load(&self, skill_name: &str) -> anyhow::Result<Value> {
        let skills = self.discover_skills();
        let skill = skills
            .iter()
            .find(|s| s.frontmatter.name == skill_name)
            .ok_or_else(|| anyhow::anyhow!("skill not found: {skill_name}"))?;

        match skill.frontmatter.context {
            SkillContext::Inject => Ok(json!({
                "skill_name": skill_name,
                "context": "inject",
                "body": skill.body,
            })),
            SkillContext::Fork => {
                let runner = self.runner.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "fork skill '{skill_name}' requires a SubagentRunner, \
                         but none is configured — call with_runner() before loading fork skills"
                    )
                })?;

                let req = SpawnRequest {
                    instruction: skill.body.clone(),
                    context_depth: ContextDepth::None,
                    context_scope: ContextScope::Conversation,
                    context: vec![],
                    session_id: None,
                };

                let result = runner.run(req).await?;

                Ok(json!({
                    "skill_name": skill_name,
                    "context": "fork",
                    "result": result,
                }))
            }
        }
    }

    /// Internal async dispatcher — all `?` operators use `anyhow::Error`.
    ///
    /// This exists so that `execute()` (which returns `ToolError`) can
    /// delegate to a method where `?` naturally propagates `anyhow::Error`.
    async fn dispatch(&self, input: &Value) -> anyhow::Result<Value> {
        let op = input
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("list");

        match op {
            "list" => Ok(self.handle_list()),
            "search" => {
                let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
                Ok(self.handle_search(query))
            }
            "info" => {
                let skill_name = input
                    .get("skill_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("skill_name is required for 'info' operation")
                    })?;
                self.handle_info(skill_name)
            }
            "load" => {
                let skill_name = input
                    .get("skill_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("skill_name is required for 'load' operation")
                    })?;
                self.handle_load(skill_name).await
            }
            other => Err(anyhow::anyhow!("unknown operation: '{other}'")),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

impl Tool for SkillEngine {
    fn name(&self) -> &str {
        "load_skill"
    }

    fn description(&self) -> &str {
        "List, search, get info about, or load Amplifier skills from the skill library"
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "operation".to_string(),
            json!({
                "type": "string",
                "enum": ["list", "search", "info", "load"],
                "description": "Operation to perform: list (all skills), search (filter by query), info (frontmatter + 200-char preview), load (inject body or fork sub-agent)",
            }),
        );
        properties.insert(
            "skill_name".to_string(),
            json!({
                "type": "string",
                "description": "Name of the skill — required for 'info' and 'load' operations",
            }),
        );
        properties.insert(
            "query".to_string(),
            json!({
                "type": "string",
                "description": "Search keyword — required for 'search' operation; case-insensitive match against name or description",
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["operation"]));

        ToolSpec {
            name: "load_skill".to_string(),
            parameters,
            description: Some(
                "List, search, get info about, or load Amplifier skills from the skill library"
                    .to_string(),
            ),
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(async move {
            match self.dispatch(&input).await {
                Ok(value) => Ok(ToolResult {
                    success: true,
                    output: Some(value),
                    error: None,
                }),
                Err(e) => Err(ToolError::ExecutionFailed {
                    message: e.to_string(),
                    stdout: None,
                    stderr: None,
                    exit_code: None,
                }),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read the home directory from the `HOME` environment variable.
fn dirs_next_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Test-only constructor — creates an engine with ONLY <base>/skills/ on the
    // search path, preventing $HOME/.amplifier/skills/ from polluting results.
    // -----------------------------------------------------------------------

    impl SkillEngine {
        fn new_isolated(vault_path: impl Into<PathBuf>) -> Self {
            let vault_path: PathBuf = vault_path.into();
            Self {
                search_paths: vec![vault_path.join("skills")],
                runner: None,
            }
        }
    }

    // --- Test helpers ---

    /// Create a skill directory under `<base_dir>/skills/<skill_name>/SKILL.md`.
    fn make_temp_skill(base_dir: &TempDir, skill_name: &str, content: &str) {
        let skill_dir = base_dir.path().join("skills").join(skill_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    const GIT_WORKFLOW_SKILL: &str = "\
---
name: git-workflow
description: A git workflow skill for version control
context: inject
user_invocable: true
---
Use conventional commits and always rebase before pushing.
";

    const CODE_REVIEW_SKILL: &str = "\
---
name: code-review
description: A code review skill for quality assurance
context: inject
user_invocable: false
---
Review all code for quality, security, and style.
";

    const FORK_SKILL_CONTENT: &str = "\
---
name: fork-analysis
description: A skill that forks a subagent for deep analysis
context: fork
user_invocable: false
---
Analyze this codebase deeply and report findings.
";

    // --- Mock runner ---

    #[allow(dead_code)]
    struct MockRunner {
        response: String,
    }

    #[async_trait::async_trait]
    impl SubagentRunner for MockRunner {
        async fn run(&self, _req: SpawnRequest) -> anyhow::Result<String> {
            Ok(self.response.clone())
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: get_spec_name_is_load_skill
    // -----------------------------------------------------------------------

    #[test]
    fn get_spec_name_is_load_skill() {
        let engine = SkillEngine::new("/some/path");
        let spec = engine.get_spec();
        assert_eq!(spec.name, "load_skill");
    }

    // -----------------------------------------------------------------------
    // Test 2: discover_skills_empty_for_nonexistent_path
    // -----------------------------------------------------------------------

    #[test]
    fn discover_skills_empty_for_nonexistent_path() {
        // Use isolated engine so $HOME/.amplifier/skills/ is excluded.
        let engine = SkillEngine::new_isolated("/this/path/does/not/exist/99999");
        let skills = engine.discover_skills();
        assert!(
            skills.is_empty(),
            "expected no skills for nonexistent path, got: {}",
            skills.len()
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: discover_skills_finds_skill_md_files
    // -----------------------------------------------------------------------

    #[test]
    fn discover_skills_finds_skill_md_files() {
        let tmp = TempDir::new().unwrap();
        make_temp_skill(&tmp, "git-workflow", GIT_WORKFLOW_SKILL);
        make_temp_skill(&tmp, "code-review", CODE_REVIEW_SKILL);

        let engine = SkillEngine::new_isolated(tmp.path());
        let skills = engine.discover_skills();

        assert_eq!(skills.len(), 2, "expected 2 skills, found {}", skills.len());

        let names: Vec<&str> = skills.iter().map(|s| s.frontmatter.name.as_str()).collect();
        assert!(
            names.contains(&"git-workflow"),
            "missing git-workflow skill"
        );
        assert!(names.contains(&"code-review"), "missing code-review skill");
    }

    // -----------------------------------------------------------------------
    // Test 4: handle_list_returns_all_skills
    // -----------------------------------------------------------------------

    #[test]
    fn handle_list_returns_all_skills() {
        let tmp = TempDir::new().unwrap();
        make_temp_skill(&tmp, "git-workflow", GIT_WORKFLOW_SKILL);
        make_temp_skill(&tmp, "code-review", CODE_REVIEW_SKILL);

        let engine = SkillEngine::new_isolated(tmp.path());
        let result = engine.handle_list();

        let arr = result.as_array().expect("expected JSON array");
        assert_eq!(arr.len(), 2, "expected 2 skills in list");

        let git_skill = arr
            .iter()
            .find(|v| v["name"] == "git-workflow")
            .expect("git-workflow not found in list");
        assert_eq!(git_skill["context"], "inject");
        assert_eq!(git_skill["user_invocable"], true);

        let review_skill = arr
            .iter()
            .find(|v| v["name"] == "code-review")
            .expect("code-review not found in list");
        assert_eq!(review_skill["context"], "inject");
        assert_eq!(review_skill["user_invocable"], false);
    }

    // -----------------------------------------------------------------------
    // Test 5: handle_search_filters_by_keyword
    // -----------------------------------------------------------------------

    #[test]
    fn handle_search_filters_by_keyword() {
        let tmp = TempDir::new().unwrap();
        make_temp_skill(&tmp, "git-workflow", GIT_WORKFLOW_SKILL);
        make_temp_skill(&tmp, "code-review", CODE_REVIEW_SKILL);

        let engine = SkillEngine::new_isolated(tmp.path());

        // "git" matches git-workflow (name)
        let result = engine.handle_search("git");
        let arr = result.as_array().expect("expected JSON array");
        assert_eq!(arr.len(), 1, "expected 1 result for 'git', got: {arr:?}");
        assert_eq!(arr[0]["name"], "git-workflow");

        // "code" matches code-review (name)
        let result = engine.handle_search("code");
        let arr = result.as_array().expect("expected JSON array");
        assert_eq!(arr.len(), 1, "expected 1 result for 'code', got: {arr:?}");
        assert_eq!(arr[0]["name"], "code-review");

        // "QUALITY" (uppercase) must still match code-review description (case-insensitive)
        let result = engine.handle_search("QUALITY");
        let arr = result.as_array().expect("expected JSON array");
        assert_eq!(
            arr.len(),
            1,
            "expected 1 result for 'QUALITY' (case-insensitive), got: {arr:?}"
        );
        assert_eq!(arr[0]["name"], "code-review");
    }

    // -----------------------------------------------------------------------
    // Test 6: handle_info_returns_frontmatter
    // -----------------------------------------------------------------------

    #[test]
    fn handle_info_returns_frontmatter() {
        let tmp = TempDir::new().unwrap();
        make_temp_skill(&tmp, "git-workflow", GIT_WORKFLOW_SKILL);

        let engine = SkillEngine::new_isolated(tmp.path());
        let result = engine
            .handle_info("git-workflow")
            .expect("should find git-workflow");

        assert_eq!(result["name"], "git-workflow");
        assert_eq!(result["context"], "inject");
        assert_eq!(result["user_invocable"], true);
        assert!(
            result.get("body_preview").is_some(),
            "body_preview field must be present"
        );

        let preview = result["body_preview"].as_str().unwrap();
        assert!(
            preview.chars().count() <= 200,
            "body_preview must be at most 200 chars, got {}",
            preview.chars().count()
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: handle_info_returns_error_for_unknown_skill
    // -----------------------------------------------------------------------

    #[test]
    fn handle_info_returns_error_for_unknown_skill() {
        let engine = SkillEngine::new_isolated("/this/path/does/not/exist/99999");
        let result = engine.handle_info("no-such-skill");
        assert!(
            result.is_err(),
            "expected error for unknown skill, got: {result:?}"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("no-such-skill") || msg.contains("not found"),
            "error should mention the skill name or 'not found', got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: handle_load_inject_returns_body
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn handle_load_inject_returns_body() {
        let tmp = TempDir::new().unwrap();
        make_temp_skill(&tmp, "git-workflow", GIT_WORKFLOW_SKILL);

        let engine = SkillEngine::new_isolated(tmp.path());
        let result = engine
            .handle_load("git-workflow")
            .await
            .expect("inject skill load should succeed");

        assert_eq!(result["skill_name"], "git-workflow");
        assert_eq!(result["context"], "inject");
        assert!(
            result.get("body").is_some(),
            "body field must be present for inject skill"
        );
        assert!(
            result["body"]
                .as_str()
                .unwrap()
                .contains("conventional commits"),
            "body should contain skill content"
        );
    }

    // -----------------------------------------------------------------------
    // Test 9: handle_load_fork_without_runner_errors_clearly
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn handle_load_fork_without_runner_errors_clearly() {
        let tmp = TempDir::new().unwrap();
        make_temp_skill(&tmp, "fork-analysis", FORK_SKILL_CONTENT);

        // No runner attached
        let engine = SkillEngine::new_isolated(tmp.path());
        let result = engine.handle_load("fork-analysis").await;

        assert!(
            result.is_err(),
            "expected error when loading fork skill without runner"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("runner") || msg.contains("fork"),
            "error should mention runner or fork requirement, got: {msg}"
        );
    }
}
