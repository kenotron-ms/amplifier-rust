    # amplifier-rust

    A Rust SDK for building multi-provider AI agent loops, compatible with the
    [Amplifier](https://github.com/microsoft/amplifier) ecosystem.

    ## What's Here

    17 crates wired together into a single streaming agent runtime:

    | Layer | Crates |
    |---|---|
    | **Providers** | `anthropic`, `openai`, `gemini`, `ollama` |
    | **Tools** | `bash`, `filesystem`, `web`, `search`, `todo`, `task`, `delegate`, `skills` |
    | **Orchestrator** | `loop-streaming` — streaming agent loop with 5-event hook system |
    | **Context** | `context-simple` — ephemeral + persistent, tiktoken compaction |
    | **Agent runtime** | `agent-runtime` — YAML frontmatter agent registry |
    | **Foundation agents** | `agent-foundation` — 6 built-in agents (explorer, zen-architect, ...) |

    ## Quick Start

    ```bash
    # Build everything
    cargo build

    # Run the agent with Anthropic (default)
    ANTHROPIC_API_KEY=sk-... ./target/debug/amplifier-android-sandbox \
      --prompt "List the files in the current directory"

    # Interactive REPL
    ANTHROPIC_API_KEY=sk-... ./target/debug/amplifier-android-sandbox

    # Other providers
    OPENAI_API_KEY=sk-...   ./target/debug/amplifier-android-sandbox --provider openai
    GEMINI_API_KEY=...      ./target/debug/amplifier-android-sandbox --provider gemini
                            ./target/debug/amplifier-android-sandbox --provider ollama --model llama3.2
    ```

    ## CLI Options

    ```
    --vault <PATH>       Vault directory for file tools  [default: ./vault]
    --provider <NAME>    anthropic | openai | gemini | ollama  [default: anthropic]
    --model <MODEL>      Override the default model
    --prompt <TEXT>      Single-turn mode: run prompt and exit
    --max-steps <N>      Max agent iterations  [default: 10]
    --sandbox            Apply OS-level restrictions (Linux only; no-op on macOS)
    ```

    ## Provider API Keys

    | Provider | Environment variable |
    |---|---|
    | Anthropic | `ANTHROPIC_API_KEY` |
    | OpenAI | `OPENAI_API_KEY` |
    | Gemini | `GEMINI_API_KEY` or `GOOGLE_API_KEY` |
    | Ollama | *(none — local)* |

    ## Development

    ```bash
    # Run all tests
    cargo test --workspace

    # Check formatting
    cargo fmt --all -- --check

    # Lint
    cargo clippy --workspace --all-targets
    ```

    Tests that require live servers (Ollama, real API keys) are marked `#[ignore]`
    and skipped by default.

    ## Sandbox

    On Linux (kernel 5.13+), `--sandbox` applies:
    - **Landlock** — restricts filesystem access to vault + `/tmp` + `/etc/ssl`
    - **Seccomp BPF** — blocks dangerous syscalls (`ptrace`, `mount`, `setuid`, ...)

    On macOS/Windows the flag is a documented no-op. Use Docker on Linux for
    full-fidelity sandbox testing:

    ```sh
    docker run --rm \
      --security-opt no-new-privileges \
      --read-only \
      -v /path/to/vault:/vault \
      amplifier-android-sandbox --sandbox --vault /vault
    ```

    ## Dependencies

    - [`amplifier-core`](https://github.com/microsoft/amplifier-core) — kernel traits
      and message types (git dependency; pulled automatically by Cargo)
    