# amplifier-module-tool-todo

Per-session todo-list tool for Amplifier agents.

Implements the `Tool` trait from `amplifier-core` for managing an in-memory,
session-scoped todo list. Items are identified by UUID v4 and carry a content
description, an active-form label, and a status string.

## Actions

- **`create`** — Replace the entire todo list with the provided items (IDs regenerated).
- **`update`** — Identical to `create`; replaces all items.
- **`list`** — Return the current todo list without modification.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-tool-todo = "0.1"
```

Construct and register the tool:

```rust
use amplifier_module_tool_todo::TodoTool;

let tool = TodoTool::default();
```

## License

MIT
