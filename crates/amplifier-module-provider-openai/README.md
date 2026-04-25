# amplifier-module-provider-openai

OpenAI Responses API (GPT) provider for Amplifier.

Implements the Provider trait from amplifier-core against the OpenAI Responses API. Supports GPT-4o, GPT-4o-mini, and reasoning models, with streaming and tool calls.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-provider-openai = "0.1"
```

Then construct a provider:

```rust
use amplifier_module_provider_openai::OpenAIProvider;
let provider = OpenAIProvider::new(std::env::var("OPENAI_API_KEY")?, "gpt-4o");
```

## License

MIT
