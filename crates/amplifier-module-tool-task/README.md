# amplifier-module-tool-task

Task primitive — subagent dispatch for Amplifier.

Implements the `Tool` trait from `amplifier-core` to provide the `spawn_agent`
tool, which enables agents to invoke sub-agents (recursive delegation) with
configurable context depth, context scope, and recursion depth limits.

## Features

- **Subagent dispatch** — agents call `spawn_agent` to delegate work to a
  child agent via the [`SubagentRunner`] trait.
- **Recursion guard** — `TaskTool` carries `current_depth` and
  `max_recursion_depth` fields; execution is rejected before the limit is
  reached, preventing unbounded recursion.
- **Context control** — callers can specify `context_depth` (`none`,
  `recent_5`, `all`) and `context_scope` (`conversation`, `agents`, `full`)
  to fine-tune what history the sub-agent receives.
- **No orchestrator coupling** — this crate has no dependency on the
  orchestrator crate, avoiding circular dependency cycles.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-tool-task = "0.1"
```

Implement the `SubagentRunner` trait and construct a `TaskTool`:

```rust
use std::sync::Arc;
use amplifier_module_tool_task::{TaskTool, SubagentRunner, SpawnRequest};

struct MyRunner;

#[async_trait::async_trait]
impl SubagentRunner for MyRunner {
    async fn run(&self, req: SpawnRequest) -> anyhow::Result<String> {
        // launch sub-agent and return its response
        Ok(format!("handled: {}", req.instruction))
    }
}

let tool = TaskTool::new(Arc::new(MyRunner), /* max_depth */ 5, /* current_depth */ 0);
```

## License

MIT
