# amplifier-module-orchestrator-loop-streaming

Streaming agent-loop orchestrator for Amplifier.

[![Crates.io](https://img.shields.io/crates/v/amplifier-module-orchestrator-loop-streaming.svg)](https://crates.io/crates/amplifier-module-orchestrator-loop-streaming)
[![Docs.rs](https://docs.rs/amplifier-module-orchestrator-loop-streaming/badge.svg)](https://docs.rs/amplifier-module-orchestrator-loop-streaming)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

## Overview

Implements Orchestrator from amplifier-core. Drives the prompt → provider stream → tool-call → tool-result loop, emitting events for UI consumers and stopping on stop_reason = end_turn.

## Usage

```rust
use amplifier_module_orchestrator_loop_streaming::{LoopOrchestrator, LoopConfig};

let orchestrator = LoopOrchestrator::new(LoopConfig {
    max_steps: Some(10),
    system_prompt: "You are a helpful assistant.".to_string(),
});
```

## License

MIT — see [LICENSE](LICENSE).
