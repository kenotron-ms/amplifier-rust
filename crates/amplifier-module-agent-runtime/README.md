# amplifier-module-agent-runtime

Agent loader & registry for Amplifier.

Reads YAML/Markdown agent definitions from disk, validates them, and exposes a
registry that the Task and Delegate tools use to look up subagents by name or
path.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-agent-runtime = "0.1"
```

Load agents from a directory of `.md` bundle files:

```rust
use amplifier_module_agent_runtime::AgentRegistry;

let mut registry = AgentRegistry::new();
let count = registry.load_from_dir(std::path::Path::new("agents/"))?;
println!("Loaded {} agents", count);

if let Some(agent) = registry.get("my-agent") {
    println!("{}: {}", agent.name, agent.description);
}
```

## Agent Bundle Format

Agent bundles are `.md` files with a YAML frontmatter block:

```markdown
---
meta:
  name: my-agent
  description: Does helpful things
tools:
  - bash
  - filesystem
---
You are a helpful agent. ...
```

## License

MIT
