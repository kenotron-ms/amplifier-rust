# amplifier-agent-foundation

Stock foundation agent definitions for Amplifier.

Provides the canonical foundation system prompts, tool bundles, and agent definitions used by Amplifier hosts that don't ship their own.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-agent-foundation = "0.1"
```

Retrieve the built-in foundation agents:

```rust
use amplifier_agent_foundation::foundation_agents;

let agents = foundation_agents();
for agent in &agents {
    println!("{}: {}", agent.name, agent.description);
}
```

## Included Agents

| Agent | Purpose |
|-------|---------|
| `explorer` | Deep local-context reconnaissance |
| `zen-architect` | Architecture, design, and code review |
| `bug-hunter` | Systematic debugging |
| `git-ops` | Git and GitHub operations |
| `modular-builder` | Implementation from complete specs |
| `security-guardian` | Security review |

## License

MIT
