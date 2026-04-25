# amplifier-module-provider-gemini

Google Gemini Developer API provider for Amplifier.

Implements the Provider trait from amplifier-core against the Gemini Developer API. Supports gemini-1.5-pro, gemini-1.5-flash, and gemini-2.0-flash with streaming.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-provider-gemini = "0.1"
```

Then construct a provider:

```rust
use amplifier_module_provider_gemini::{GeminiProvider, GeminiConfig};
let provider = GeminiProvider::new(GeminiConfig {
    api_key: std::env::var("GEMINI_API_KEY").unwrap(),
    ..GeminiConfig::default()
});
```

## License

MIT
