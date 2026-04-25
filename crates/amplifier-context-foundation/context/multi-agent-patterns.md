# Multi-Agent Patterns

    This context provides patterns for orchestrating multiple agents effectively.
    It mirrors `foundation:context/agents/multi-agent-patterns.md` from the Python amplifier-foundation bundle.

    ---

    ## Parallel Agent Dispatch

    **CRITICAL**: For non-trivial investigations, dispatch MULTIPLE agents in parallel.
    Different agents have different tools, perspectives, and context.

    ```json
    [
      {"agent": "explorer", "instruction": "Survey the authentication module structure"},
      {"agent": "zen-architect", "instruction": "Review auth module for design patterns"},
      {"agent": "bug-hunter", "instruction": "Find security issues in auth module"}
    ]
    ```

    **Why parallel matters:**
    - Each agent brings different expertise
    - Together they reveal behavior + design issues + security gaps
    - Parallel is faster than sequential for independent subtasks

    ---

    ## Complementary Agent Combinations

    | Task Type | Agent Combination | Why |
    |-----------|-------------------|-----|
    | **Code investigation** | `explorer` + `zen-architect` | Survey + design assessment |
    | **Bug debugging** | `bug-hunter` | Hypothesis-driven debugging |
    | **Implementation** | `zen-architect` → `modular-builder` → `zen-architect` | Design → implement → review |
    | **Security review** | `security-guardian` + `explorer` | Security patterns + codebase survey |
    | **Git work** | `git-ops` | Safe commits, PRs, branches |

    ---

    ## Multi-Agent Collaboration

    Use `context_scope="agents"` so later agents see earlier agents' work:

    ```json
    // Agent A works independently
    {"agent": "explorer", "instruction": "Find all auth issues", "context_depth": "none"}

    // Agent B sees Agent A's output
    {"agent": "zen-architect", "instruction": "Design fixes", "context_scope": "agents"}

    // Agent C sees both A and B
    {"agent": "modular-builder", "instruction": "Implement the fixes", "context_scope": "agents"}
    ```

    ---

    ## Task Decomposition for Implementation

    **Before delegating to modular-builder, ensure the spec is complete.**

    | Task Type | Strategy |
    |-----------|----------|
    | "Implement X from spec in [file]" | Direct to modular-builder |
    | "Add feature Y" (no spec) | zen-architect first → then modular-builder |
    | "Refactor Z" | zen-architect (plan) → modular-builder (execute) |

    **modular-builder requires:**
    - Exact file paths
    - Complete function signatures with types
    - Pattern reference or design freedom
    - Success criteria

    **Missing any? Use zen-architect first.**

    ---

    ## Creative Patterns

    ### Agent Chain with Accumulated Knowledge

    ```json
    {"agent": "explorer", "instruction": "Survey", "context_depth": "none"}
    {"agent": "zen-architect", "instruction": "Design", "context_scope": "agents"}
    {"agent": "modular-builder", "instruction": "Implement", "context_scope": "agents"}
    ```

    ### Self-Delegation for Token Management

    When your session context is filling up, spawn yourself to continue:

    ```json
    {"agent": "self", "instruction": "Continue the analysis in depth",
     "context_depth": "all", "context_scope": "full"}
    ```

    **Recommended for self-delegation:** `context_depth="all", context_scope="full"` —
    the sub-instance should see everything to avoid re-doing work.

    ### Parallel Investigation with Synthesis

    ```json
    // Parallel surveys
    {"agent": "explorer", "instruction": "Check frontend", "context_depth": "none"}
    {"agent": "explorer", "instruction": "Check backend", "context_depth": "none"}

    // Synthesizer sees all results
    {"agent": "zen-architect", "instruction": "Synthesize findings", "context_scope": "agents"}
    ```
    