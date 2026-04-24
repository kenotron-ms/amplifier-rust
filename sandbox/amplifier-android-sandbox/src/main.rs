#![allow(unused_imports)]

use clap::Parser;
use std::path::PathBuf;

mod hooks;
mod sandbox;
mod tools;

#[derive(Parser)]
#[command(name = "amplifier-android-sandbox", about = "Amplifier Android Sandbox agent runner")]
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    eprintln!(
        "amplifier-android-sandbox starting: provider={}, vault={}",
        args.provider,
        args.vault.display()
    );
    if args.sandbox {
        sandbox::apply(&args.vault)?;
    }
    std::fs::create_dir_all(&args.vault)?;
    eprintln!("[sandbox] skeleton OK — full wiring in Task 14");
    Ok(())
}
