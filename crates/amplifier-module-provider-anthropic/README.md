# amplifier-module-provider-anthropic

Anthropic Messages API (Claude) provider for Amplifier.

Implements the Provider trait from amplifier-core against the Anthropic Messages API. Supports Claude 3.5 Sonnet, Haiku, and Opus models, with streaming via Server-Sent Events and tool-use round-trips.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-provider-anthropic = "0.1"
```

Then construct a provider:

```rust
use amplifier_module_provider_anthropic::AnthropicProvider;

let provider = AnthropicProvider::new(std::env::var("ANTHROPIC_API_KEY")?, "claude-3-5-sonnet-20241022");
```

## License

MIT
