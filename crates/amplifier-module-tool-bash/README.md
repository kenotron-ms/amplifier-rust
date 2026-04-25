# amplifier-module-tool-bash

Sandboxed bash command execution tool for Amplifier agents.

Implements the `Tool` trait from `amplifier-core` for executing shell commands
with configurable safety profiles, vault-root scoping, timeout enforcement, and
platform-conditional sandboxing.

## Features

- **Vault-root scoping** — the working directory is pinned to the agent vault
  root, preventing commands from escaping the configured workspace.
- **Timeout enforcement** — every command runs under a configurable deadline
  (default 30 s). Commands that exceed the deadline are killed and an error is
  returned, so agents never block indefinitely on runaway processes.
- **Platform-conditional sandboxing** — the `SafetyProfile` enum selects from
  five enforcement levels at construction time:
  - `Android` — allowlist-based; only [toybox](https://landley.net/toybox/)
    commands present in AOSP are permitted (designed for sandboxed Android
    environments).
  - `Strict` — denylist blocking destructive and privileged commands.
  - `Standard` — denylist blocking privileged escalation commands.
  - `Permissive` — minimal denylist for lightly controlled environments.
  - `Unrestricted` — no restrictions; all commands are permitted.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-tool-bash = "0.1"
```

Construct a tool:

```rust
use amplifier_module_tool_bash::{BashConfig, BashTool, SafetyProfile};

let config = BashConfig {
    safety_profile: SafetyProfile::Strict,
    ..Default::default()
};
let tool = BashTool::new(config);
```

## License

MIT
