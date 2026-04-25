//! Built-in foundation agents for the Amplifier agent framework.
//!
//! Provides six general-purpose agents available in every Amplifier environment:
//! - `explorer` — deep local-context reconnaissance
//! - `zen-architect` — architecture, design, and code review
//! - `bug-hunter` — systematic debugging
//! - `git-ops` — git and GitHub operations
//! - `modular-builder` — implementation from complete specs
//! - `security-guardian` — security review

use amplifier_module_agent_runtime::AgentConfig;

/// Returns the six built-in foundation agents.
///
/// These agents provide the core set of general-purpose capabilities available
/// in every Amplifier environment. They are loaded at the lowest priority —
/// vault-local and user-global agents override them by name.
pub fn foundation_agents() -> Vec<AgentConfig> {
    vec![
        // 1. explorer
        AgentConfig {
            name: "explorer".to_string(),
            description: "Deep local-context reconnaissance agent. Surveys codebases, docs, and \
                          configs. Use for multi-file exploration tasks."
                .to_string(),
            tools: vec![
                "filesystem".to_string(),
                "search".to_string(),
                "bash".to_string(),
                "web".to_string(),
            ],
            instruction: "You are an expert at exploring codebases. Your job is to perform \
                          comprehensive surveys of local code, documentation, configuration, and \
                          user-provided content. Conduct structured sweeps across relevant \
                          packages and summarize your findings clearly. Always read multiple files \
                          before drawing conclusions."
                .to_string(),
        },
        // 2. zen-architect
        AgentConfig {
            name: "zen-architect".to_string(),
            description: "Architecture, design, and code review. Modes: ANALYZE, ARCHITECT, \
                          REVIEW. Embodies ruthless simplicity."
                .to_string(),
            tools: vec!["filesystem".to_string(), "search".to_string()],
            instruction: "You are an expert software architect with a philosophy of ruthless \
                          simplicity. You operate in three modes: ANALYZE (break down problems \
                          and design solutions), ARCHITECT (system design and module \
                          specification), and REVIEW (code quality assessment). You create \
                          specifications that implementers can execute. You ask: is this the \
                          simplest solution that could work?"
                .to_string(),
        },
        // 3. bug-hunter
        AgentConfig {
            name: "bug-hunter".to_string(),
            description: "Systematic debugging specialist. Hypothesis-driven. Use when errors, \
                          test failures, or unexpected behavior occurs."
                .to_string(),
            tools: vec![
                "filesystem".to_string(),
                "search".to_string(),
                "bash".to_string(),
            ],
            instruction: "You are a systematic debugging specialist. You investigate bugs using \
                          a hypothesis-driven approach: form a hypothesis, gather evidence, test \
                          the hypothesis, narrow the cause. You identify root causes without \
                          adding unnecessary complexity. You write minimal fixes that address the \
                          actual problem."
                .to_string(),
        },
        // 4. git-ops
        AgentConfig {
            name: "git-ops".to_string(),
            description: "Git and GitHub operations. Commits, PRs, branches. Enforces \
                          conventional commits and safety protocols."
                .to_string(),
            tools: vec!["bash".to_string(), "filesystem".to_string()],
            instruction: "You are a git operations specialist. You handle commits, pull \
                          requests, branches, merges, and GitHub operations. You enforce \
                          conventional commit message format, ensure atomic commits, and follow \
                          safety protocols (no force pushes to main, no destructive operations \
                          without explicit confirmation)."
                .to_string(),
        },
        // 5. modular-builder
        AgentConfig {
            name: "modular-builder".to_string(),
            description: "Implementation-only agent. Requires complete spec with file paths, \
                          interfaces, and criteria. Will stop and ask if spec is incomplete."
                .to_string(),
            tools: vec![
                "filesystem".to_string(),
                "search".to_string(),
                "bash".to_string(),
            ],
            instruction: "You are an implementation specialist. You ONLY implement from complete \
                          specifications. A complete spec must include: file paths, function \
                          signatures with types, pattern reference or design freedom, and success \
                          criteria. If ANY of these are missing, you STOP and ask for the missing \
                          information — you never guess at intent. You follow TDD and write \
                          minimal implementations."
                .to_string(),
        },
        // 6. security-guardian
        AgentConfig {
            name: "security-guardian".to_string(),
            description: "Security review specialist. OWASP Top 10, hardcoded secrets, input \
                          validation, cryptography, dependency vulnerabilities."
                .to_string(),
            tools: vec![
                "filesystem".to_string(),
                "search".to_string(),
                "bash".to_string(),
            ],
            instruction: "You are a security specialist. You review code for security \
                          vulnerabilities including OWASP Top 10, hardcoded secrets, \
                          input/output validation gaps, cryptographic weaknesses, and dependency \
                          vulnerabilities. You are a required checkpoint before production \
                          deployments."
                .to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foundation_agents_returns_six_agents() {
        let agents = foundation_agents();
        assert_eq!(agents.len(), 6, "expected 6 foundation agents");
    }

    #[test]
    fn all_foundation_agents_have_non_empty_names() {
        for agent in foundation_agents() {
            assert!(!agent.name.is_empty(), "agent name must not be empty");
            assert!(
                !agent.description.is_empty(),
                "agent description must not be empty"
            );
            assert!(
                !agent.instruction.is_empty(),
                "agent instruction must not be empty"
            );
        }
    }

    #[test]
    fn foundation_agent_names_are_expected() {
        let names: Vec<String> = foundation_agents().into_iter().map(|a| a.name).collect();
        for expected in &[
            "explorer",
            "zen-architect",
            "bug-hunter",
            "git-ops",
            "modular-builder",
            "security-guardian",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing foundation agent: {expected}"
            );
        }
    }
}
