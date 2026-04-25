# amplifier-module-tool-web

WebFetch and WebSearch tools for Amplifier agents.

Implements the `Tool` trait from `amplifier-core` for fetching web content and
performing web searches from within Amplifier agents.

## Features

- **`fetch_url`** — fetch the content of a URL (HTML, JSON, plain text) and
  return it as a string suitable for agent consumption.
- **`search_web`** — perform a web search query and return a list of result
  snippets that the agent can reason over.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-tool-web = "0.1"
```

Register the tools with your agent runtime:

```rust
use amplifier_module_tool_web::WebToolSuite;

let tools = WebToolSuite::tools(); // Vec<(String, Arc<dyn Tool>)>
```

## License

MIT
