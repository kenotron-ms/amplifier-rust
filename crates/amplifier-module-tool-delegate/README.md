# amplifier-module-tool-delegate

Hierarchical delegation tool for Amplifier.

Builds on `amplifier-module-tool-task` to add depth-tracked subagent invocation
with optional context inheritance and namespace-scoped name resolution
(`self`, `namespace:path`, registry).

## Features

- **Depth-tracked delegation** — carries `current_depth` through each
  invocation and rejects requests that would exceed the configured
  `max_self_delegation_depth`, preventing unbounded recursion.
- **Context inheritance** — callers can forward conversation history to
  child agents via `context_depth` and `context_scope` parameters.
- **Namespace-scoped name resolution** — agent names are resolved using a
  three-tier strategy: the special `self` token re-invokes the current agent,
  `namespace:path` strings are resolved through the
  [`AgentRegistry`], and bare names are looked up in the same registry.
- **No orchestrator coupling** — depends on `amplifier-module-agent-runtime`
  for the registry abstraction but not on any orchestrator crate, avoiding
  circular dependency cycles.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-tool-delegate = "0.1"
```

Implement the `SubagentRunner` trait (re-exported from
`amplifier-module-tool-task`) and construct a `DelegateTool`:

```rust
use std::sync::Arc;
use amplifier_module_tool_delegate::{DelegateTool, DelegateConfig, SubagentRunner, SpawnRequest};
use amplifier_module_agent_runtime::AgentRegistry;

struct MyRunner;

#[async_trait::async_trait]
impl SubagentRunner for MyRunner {
    async fn run(&self, req: SpawnRequest) -> anyhow::Result<String> {
        Ok(format!("handled: {}", req.instruction))
    }
}

let registry = Arc::new(AgentRegistry::default());
let tool = DelegateTool::new(
    Arc::new(MyRunner),
    registry,
    DelegateConfig::default(),
);
```

## License

MIT
