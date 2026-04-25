//! Amplifier Android Sandbox — main entry point.
//!
//! ## Startup sequence
//!
//! 1. Parse CLI arguments (`Args`).
//! 2. If `--sandbox`, apply OS-level restrictions via `sandbox::apply`.
//! 3. Create the vault directory with `std::fs::create_dir_all`.
//! 4. Build the hook registry via `hooks::build_registry`.
//! 5. Build the base tool map via `tools::build_registry`.
//! 6. Create the [`LoopOrchestrator`] with `max_steps` from CLI args.
//! 7. Wire [`TaskTool`] into the tool map, backed by the orchestrator as [`SubagentRunner`].
//! 8. Wire [`SkillEngine`] into the tool map, backed by the vault path;
//!    ensure `<vault>/skills/` directory exists.
//! 9. Build the provider from `--provider` (reading the appropriate API-key env var),
//!    register it and all tools with the orchestrator, then either execute the
//!    single `--prompt` or run the interactive REPL.

use anyhow::{Context, Result};
use clap::Parser;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use amplifier_core::traits::Provider;
use amplifier_module_context_simple::SimpleContext;
use amplifier_module_orchestrator_loop_streaming::{LoopConfig, LoopOrchestrator};
use amplifier_module_provider_anthropic::{AnthropicConfig, AnthropicProvider};
use amplifier_module_provider_gemini::{GeminiConfig, GeminiProvider};
use amplifier_module_provider_ollama::{OllamaConfig, OllamaProvider};
use amplifier_module_provider_openai::{OpenAIConfig, OpenAIProvider};
use amplifier_module_tool_skills::SkillEngine;
use amplifier_module_tool_task::{SubagentRunner, TaskTool};

mod hooks;
mod sandbox;
mod tools;

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "amplifier-android-sandbox",
    about = "Amplifier Android Sandbox agent runner"
)]
struct Args {
    /// Path to the vault directory
    #[arg(long, default_value = "./vault")]
    vault: PathBuf,

    /// Provider to use (anthropic, ollama, gemini, openai)
    #[arg(long, default_value = "anthropic")]
    provider: String,

    /// Model override
    #[arg(long)]
    model: Option<String>,

    /// Prompt to run
    #[arg(long)]
    prompt: Option<String>,

    /// Enable sandbox restrictions (Linux only)
    #[arg(long, default_value_t = false)]
    sandbox: bool,

    /// Maximum number of agent steps
    #[arg(long, default_value_t = 10)]
    max_steps: usize,
}

// ---------------------------------------------------------------------------
// Provider builder
// ---------------------------------------------------------------------------

/// Build a boxed [`Provider`] for the given `provider_name`.
///
/// # API key requirements
///
/// | Provider   | Required env var(s)                   |
/// |------------|---------------------------------------|
/// | anthropic  | `ANTHROPIC_API_KEY`                   |
/// | gemini     | `GEMINI_API_KEY` or `GOOGLE_API_KEY`  |
/// | openai     | `OPENAI_API_KEY`                      |
/// | ollama     | *(none)*                              |
///
/// Returns an error if a required env var is missing, or if `provider_name`
/// is not one of the four supported values.
fn build_provider(provider_name: &str, model: Option<&str>) -> Result<Box<dyn Provider>> {
    match provider_name {
        "anthropic" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY").with_context(|| {
                "ANTHROPIC_API_KEY environment variable is required for the anthropic provider"
            })?;
            let mut config = AnthropicConfig {
                api_key,
                ..AnthropicConfig::default()
            };
            if let Some(m) = model {
                config.model = m.to_string();
            }
            Ok(Box::new(AnthropicProvider::new(config)))
        }
        "gemini" => {
            let api_key = std::env::var("GEMINI_API_KEY")
                .or_else(|_| std::env::var("GOOGLE_API_KEY"))
                .with_context(|| {
                    "GEMINI_API_KEY or GOOGLE_API_KEY environment variable is required \
                     for the gemini provider"
                })?;
            let mut config = GeminiConfig {
                api_key,
                ..GeminiConfig::default()
            };
            if let Some(m) = model {
                config.model = m.to_string();
            }
            Ok(Box::new(GeminiProvider::new(config)))
        }
        "openai" => {
            let api_key = std::env::var("OPENAI_API_KEY").with_context(|| {
                "OPENAI_API_KEY environment variable is required for the openai provider"
            })?;
            let mut config = OpenAIConfig {
                api_key,
                ..OpenAIConfig::default()
            };
            if let Some(m) = model {
                config.model = m.to_string();
            }
            Ok(Box::new(OpenAIProvider::new(config)))
        }
        "ollama" => {
            let model_name = model.unwrap_or("llama3.2").to_string();
            let config = OllamaConfig {
                model: model_name,
                ..OllamaConfig::default()
            };
            Ok(Box::new(OllamaProvider::new(config)))
        }
        other => {
            anyhow::bail!(
                "unknown provider '{}'; valid options are: anthropic, gemini, openai, ollama",
                other
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Step 1: parse args
    let args = Args::parse();

    // Step 2: apply sandbox restrictions (Linux only; no-op elsewhere)
    if args.sandbox {
        sandbox::apply(&args.vault)?;
    }

    // Step 3: create the vault directory
    std::fs::create_dir_all(&args.vault)
        .with_context(|| format!("failed to create vault directory: {}", args.vault.display()))?;

    // Step 4: build the hook registry
    let hook_registry = hooks::build_registry();

    // Step 5: build the base tool map (9 core tools; TaskTool + SkillEngine added below)
    let mut tool_map = tools::build_registry(&args.vault)?;

    // Step 6: create the orchestrator
    let orch = Arc::new(LoopOrchestrator::new(LoopConfig {
        max_steps: Some(args.max_steps),
        ..LoopConfig::default()
    }));

    // Step 7: wire TaskTool (backed by the orchestrator as SubagentRunner)
    let task_tool = TaskTool::new(
        Arc::clone(&orch) as Arc<dyn SubagentRunner>,
        5, // max_recursion_depth
        0, // current_depth (top-level = 0)
    );
    tool_map.insert("task".to_string(), Box::new(task_tool));

    // Step 8: wire SkillEngine; ensure the skills directory exists
    let skills_dir = args.vault.join("skills");
    std::fs::create_dir_all(&skills_dir)?;
    let skills_tool = SkillEngine::new(&args.vault);
    tool_map.insert("skills".to_string(), Box::new(skills_tool));

    // Step 9: build the provider, register it, and register all tools
    let provider: Box<dyn Provider> = build_provider(&args.provider, args.model.as_deref())?;
    orch.register_provider(args.provider.clone(), Arc::from(provider))
        .await;

    for (_name, boxed_tool) in tool_map {
        // Box<dyn Tool + Send + Sync> → Arc<dyn Tool + Send + Sync>
        // The coercion is valid because Tool: Send + Sync.
        let arc_tool: Arc<dyn amplifier_core::traits::Tool + Send + Sync> = Arc::from(boxed_tool);
        orch.register_tool(arc_tool).await;
    }

    // Build the in-memory context
    let mut context = SimpleContext::new(vec![]);

    if let Some(prompt) = args.prompt {
        // Single-turn mode: execute once and print the response
        let response = orch
            .execute(prompt, &mut context, &hook_registry, |_token| {})
            .await?;
        println!("{response}");
    } else {
        // Interactive REPL mode
        eprintln!("[sandbox] REPL mode — type your prompt, Ctrl-D to exit");
        let stdin = io::stdin();
        loop {
            print!("> ");
            let _ = io::stdout().flush();

            let mut line = String::new();
            match stdin.read_line(&mut line) {
                Ok(0) => break, // EOF (Ctrl-D)
                Err(e) => {
                    eprintln!("[error] failed to read input: {e}");
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match orch
                        .execute(trimmed, &mut context, &hook_registry, |_token| {})
                        .await
                    {
                        Ok(response) => println!("{response}"),
                        Err(e) => eprintln!("[error] {e}"),
                    }
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// An unknown provider name must produce an error whose message explains
    /// which provider names are valid.
    #[test]
    fn unknown_provider_yields_clear_error() {
        let result = build_provider("not_a_real_provider", None);
        assert!(result.is_err(), "unknown provider should return Err");
        match result {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("not_a_real_provider") || msg.contains("valid"),
                    "error should mention the invalid name or list valid options, got: {msg}"
                );
            }
            Ok(_) => unreachable!(),
        }
    }

    /// anthropic provider requires ANTHROPIC_API_KEY.
    #[test]
    fn anthropic_requires_api_key() {
        // Unset the key so this test is deterministic regardless of environment.
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
        match build_provider("anthropic", None) {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("ANTHROPIC_API_KEY"),
                    "error should mention ANTHROPIC_API_KEY, got: {msg}"
                );
            }
            Ok(_) => panic!("anthropic should fail without ANTHROPIC_API_KEY"),
        }
    }

    /// gemini provider requires GEMINI_API_KEY or GOOGLE_API_KEY.
    #[test]
    fn gemini_requires_api_key() {
        unsafe {
            std::env::remove_var("GEMINI_API_KEY");
            std::env::remove_var("GOOGLE_API_KEY");
        }
        match build_provider("gemini", None) {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("GEMINI_API_KEY") || msg.contains("GOOGLE_API_KEY"),
                    "error should mention GEMINI_API_KEY or GOOGLE_API_KEY, got: {msg}"
                );
            }
            Ok(_) => panic!("gemini should fail without GEMINI_API_KEY or GOOGLE_API_KEY"),
        }
    }

    /// openai provider requires OPENAI_API_KEY.
    #[test]
    fn openai_requires_api_key() {
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        match build_provider("openai", None) {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("OPENAI_API_KEY"),
                    "error should mention OPENAI_API_KEY, got: {msg}"
                );
            }
            Ok(_) => panic!("openai should fail without OPENAI_API_KEY"),
        }
    }

    /// ollama does not require any API key.
    #[test]
    fn ollama_needs_no_api_key() {
        assert!(
            build_provider("ollama", None).is_ok(),
            "ollama should succeed without any API key"
        );
    }
}
