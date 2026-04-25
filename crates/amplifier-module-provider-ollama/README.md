# amplifier-module-provider-ollama

Ollama / OpenAI-compatible local-LLM provider for Amplifier.

Implements the Provider trait via the OpenAI-compatible ChatCompletions endpoint exposed by Ollama, llama.cpp's llama-server, vLLM, and similar runtimes. Defaults to http://localhost:11434.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
amplifier-module-provider-ollama = "0.1"
```

Then construct a provider:

```rust
use amplifier_module_provider_ollama::{OllamaConfig, OllamaProvider};

let provider = OllamaProvider::new(OllamaConfig {
    model: "llama3.2".to_string(),
    ..Default::default()
});
```

## License

MIT
