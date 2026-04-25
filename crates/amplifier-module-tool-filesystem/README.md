# amplifier-module-tool-filesystem

Vault-root-scoped Read/Write/Edit/Glob filesystem tools for Amplifier.

Implements the `Tool` trait from `amplifier-core` for reading, writing, editing, and
searching files, with all operations scoped to a configured vault root.

## Features

- **Read** — reads file contents within the vault root, with optional line offsets and limits.
- **Write** — writes file contents to allowed write paths within the vault root.
- **Edit** — performs targeted string replacements within files, with single or replace-all modes.
- **Glob** — matches files using glob patterns relative to the vault root.
- **Grep** — searches file contents using regex patterns, with optional context lines.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-tool-filesystem = "0.1"
```

Construct the tools:

```rust
use amplifier_module_tool_filesystem::{FilesystemConfig, ReadFileTool, WriteFileTool, EditFileTool, GlobTool};
use std::path::PathBuf;

let config = FilesystemConfig::new(PathBuf::from("/path/to/vault"));
let read_tool = ReadFileTool::new(config.clone());
let write_tool = WriteFileTool::new(config.clone());
```

## License

MIT
