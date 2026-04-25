# amplifier-module-context-simple

Simple context-window manager with tiktoken-based truncation.

[![Crates.io](https://img.shields.io/crates/v/amplifier-module-context-simple.svg)](https://crates.io/crates/amplifier-module-context-simple)
[![Docs.rs](https://docs.rs/amplifier-module-context-simple/badge.svg)](https://docs.rs/amplifier-module-context-simple)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

## Overview

Implements the ContextManager trait from amplifier-core. Maintains a rolling
conversation history, counts tokens with tiktoken-rs, and truncates from the
head when the configured limit is exceeded — preserving the system message.

## Usage

```rust
use amplifier_module_context_simple::SimpleContext;

let ctx = SimpleContext::new(8192); // max tokens
```

## Features

- **Rolling history** – persisted across turns via `add_message` / `push_turn` / `set_messages` / `clear`.
- **Ephemeral buffer** – messages added with `push_ephemeral` are included in the next provider call only, then discarded.
- **Token counting** – `token_count()` uses the `cl100k_base` encoding from `tiktoken-rs`.
- **Automatic compaction** – `compact_if_needed(threshold)` drops the oldest 50 % of messages in a loop until the token count is within budget.
- **`ContextManager` impl** – fulfils the `amplifier_core::traits::ContextManager` contract; plug directly into any Amplifier orchestrator.

## License

MIT — see [LICENSE](LICENSE).
