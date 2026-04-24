//! SKILL.md parser — frontmatter + body.
//!
//! Parses SKILL.md files with YAML frontmatter delimited by `---` on own lines.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where the skill's context is injected when invoked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SkillContext {
    /// Inject the skill body into the current conversation context.
    #[default]
    Inject,
    /// Fork a new agent context for the skill.
    Fork,
}

/// Parsed YAML frontmatter from a SKILL.md file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Unique name of the skill.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// How the skill integrates with the conversation context.
    #[serde(default)]
    pub context: SkillContext,
    /// Whether the skill is user-invocable via `/command`.
    #[serde(default)]
    pub user_invocable: bool,
    /// Directory where the skill file lives; populated by the loader, not parsed from YAML.
    #[serde(skip)]
    pub directory: PathBuf,
}

/// A fully parsed SKILL.md file.
#[derive(Debug, Clone)]
pub struct ParsedSkill {
    /// Parsed YAML frontmatter.
    pub frontmatter: SkillFrontmatter,
    /// Markdown body content (everything after the closing `---`), trimmed.
    pub body: String,
}

/// Parse a SKILL.md file from its string content.
///
/// # Errors
/// Returns an error if:
/// - The frontmatter `---` delimiters are missing or incomplete.
/// - The YAML is syntactically invalid.
/// - A required field (e.g., `name`) is absent from the frontmatter.
pub fn parse_skill_md(content: &str) -> anyhow::Result<ParsedSkill> {
    // Strip UTF-8 BOM if present.
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);

    // Scan lines for two `---` delimiters.
    let lines: Vec<&str> = content.lines().collect();
    let mut first_delim: Option<usize> = None;
    let mut second_delim: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "---" {
            match first_delim {
                None => first_delim = Some(i),
                Some(_) => {
                    second_delim = Some(i);
                    break;
                }
            }
        }
    }

    let (first, second) = match (first_delim, second_delim) {
        (Some(f), Some(s)) => (f, s),
        _ => anyhow::bail!(
            "SKILL.md is missing frontmatter delimiters '---'; \
             expected two lines containing only '---'"
        ),
    };

    // YAML lives between the two delimiters.
    let yaml = lines[first + 1..second].join("\n");

    // Body lives after the second delimiter, trimmed.
    let body = lines[second + 1..].join("\n").trim().to_string();

    // Parse YAML into typed frontmatter.
    let frontmatter: SkillFrontmatter = serde_yaml::from_str(&yaml)
        .map_err(|e| anyhow::anyhow!("Failed to parse SKILL.md frontmatter YAML: {e}"))?;

    Ok(ParsedSkill { frontmatter, body })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_SKILL: &str = "\
---
name: simple-skill
description: A simple test skill for unit testing
context: inject
user_invocable: false
---
This is the body of the skill.
It has multiple lines.

## Section

Some content here.
";

    const FORK_SKILL: &str = "\
---
name: fork-skill
description: A skill that uses fork context
context: fork
user_invocable: true
---
Fork skill body content.
";

    const MINIMAL_SKILL: &str = "\
---
name: minimal-skill
description: A minimal skill with only required fields
---
Minimal body.
";

    #[test]
    fn parse_simple_skill_name_and_description() {
        let parsed = parse_skill_md(SIMPLE_SKILL).expect("should parse successfully");
        assert_eq!(parsed.frontmatter.name, "simple-skill");
        assert_eq!(
            parsed.frontmatter.description,
            "A simple test skill for unit testing"
        );
    }

    #[test]
    fn parse_simple_skill_context_inject() {
        let parsed = parse_skill_md(SIMPLE_SKILL).expect("should parse successfully");
        assert_eq!(parsed.frontmatter.context, SkillContext::Inject);
        assert!(!parsed.frontmatter.user_invocable);
    }

    #[test]
    fn parse_fork_skill_context_is_fork() {
        let parsed = parse_skill_md(FORK_SKILL).expect("should parse successfully");
        assert_eq!(parsed.frontmatter.context, SkillContext::Fork);
        assert!(parsed.frontmatter.user_invocable);
    }

    #[test]
    fn parse_extracts_body() {
        let parsed = parse_skill_md(SIMPLE_SKILL).expect("should parse successfully");
        assert!(
            parsed.body.contains("This is the body of the skill."),
            "body missing first line"
        );
        assert!(
            parsed.body.contains("It has multiple lines."),
            "body missing second line"
        );
        assert!(parsed.body.contains("## Section"), "body missing heading");
        assert!(
            parsed.body.contains("Some content here."),
            "body missing content"
        );
    }

    #[test]
    fn parse_minimal_skill_defaults_context_to_inject() {
        let parsed = parse_skill_md(MINIMAL_SKILL).expect("should parse successfully");
        assert_eq!(parsed.frontmatter.context, SkillContext::Inject);
        assert!(!parsed.frontmatter.user_invocable);
    }

    #[test]
    fn parse_returns_error_for_missing_frontmatter() {
        let content = "No frontmatter here\nJust regular content\n";
        let result = parse_skill_md(content);
        assert!(result.is_err(), "expected error for missing frontmatter");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("---") || msg.contains("frontmatter") || msg.contains("delimiter"),
            "error message should mention delimiters, got: {msg}"
        );
    }

    #[test]
    fn parse_returns_error_for_invalid_yaml() {
        // Syntactically broken YAML (unmatched bracket).
        let content = "---\n: invalid: yaml: [{\n---\nbody\n";
        let result = parse_skill_md(content);
        assert!(result.is_err(), "expected error for invalid YAML");
    }

    #[test]
    fn parse_returns_error_for_missing_required_field() {
        // `name` is required; only `description` is present.
        let content = "---\ndescription: Missing name field\n---\nbody\n";
        let result = parse_skill_md(content);
        assert!(
            result.is_err(),
            "expected error when required field 'name' is missing"
        );
    }
}
