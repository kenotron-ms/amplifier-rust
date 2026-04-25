# amplifier-module-tool-search

Vault-root-scoped grep/find tool for Amplifier.

Implements the `Tool` trait from `amplifier-core` for searching file contents
with a regex pattern, with all operations scoped to a configured vault root.

## Features

- **ripgrep backend** — fast subprocess-based search using the system `rg`
  binary; parses NDJSON output (`rg --json`).
- **Pure-Rust fallback** — `walkdir` + `regex` on a blocking thread pool,
  used automatically when `rg` is not available.
- **Glob filtering** — optional filename glob (e.g. `*.rs`) to restrict
  which files are searched.
- **Result capping** — configurable `max_results` limit (default 200).

Both backends return a JSON array of `{file, line, content}` objects.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-tool-search = "0.1"
```

Construct the tool:

```rust
use amplifier_module_tool_search::{GrepCodebaseTool, SearchConfig};
use std::path::PathBuf;

let config = SearchConfig::new(PathBuf::from("/path/to/vault"));
let tool = GrepCodebaseTool::new(config);
```

## License

MIT
