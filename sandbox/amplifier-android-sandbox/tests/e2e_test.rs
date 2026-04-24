//! End-to-end smoke tests for the amplifier-android-sandbox binary.
//!
//! ## Requirements
//!
//! These tests require Ollama running on `localhost:11434` with the
//! `llama3.2` model available.  They are marked `#[ignore]` so they are
//! skipped in CI by default.
//!
//! To run them locally (with Ollama running):
//!
//! ```text
//! cargo test -p amplifier-android-sandbox -- --ignored --nocapture
//! ```

use std::process::Command;

/// Smoke test: single-turn prompt via Ollama returns the expected output.
///
/// Launches the sandbox binary with the Ollama provider and instructs it to
/// reply with the literal string `HELLO_SANDBOX`.  The test asserts that the
/// process exits successfully and that the output contains `HELLO_SANDBOX`.
#[test]
#[ignore = "requires Ollama running on localhost:11434 with llama3.2 model"]
fn e2e_ollama_single_turn_says_hello() {
    let binary = env!("CARGO_BIN_EXE_amplifier-android-sandbox");

    let output = Command::new(binary)
        .args([
            "--provider",
            "ollama",
            "--model",
            "llama3.2",
            "--prompt",
            "Reply with exactly: HELLO_SANDBOX",
            "--vault",
            "/tmp/sandbox-e2e-test",
        ])
        .output()
        .expect("failed to launch amplifier-android-sandbox binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!(
        "stdout:
{stdout}"
    );
    println!(
        "stderr:
{stderr}"
    );

    assert!(
        output.status.success(),
        "binary exited with non-zero status: {}
stderr: {stderr}",
        output.status
    );
    assert!(
        stdout.contains("HELLO_SANDBOX"),
        "expected stdout to contain 'HELLO_SANDBOX', got:
{stdout}"
    );
}

/// Smoke test: the LLM-backed bash tool rejects `sudo` commands in the
/// Android safety profile.
///
/// Launches the sandbox binary with a prompt that asks it to run
/// `sudo ls /root`.  The Android bash safety profile should block the
/// `sudo` call; the test asserts that the output contains at least one of
/// the expected refusal indicators.
#[test]
#[ignore = "requires Ollama running on localhost:11434 with llama3.2 model"]
fn e2e_android_bash_rejects_sudo_via_llm() {
    let binary = env!("CARGO_BIN_EXE_amplifier-android-sandbox");

    let output = Command::new(binary)
        .args([
            "--provider",
            "ollama",
            "--model",
            "llama3.2",
            "--prompt",
            "Run this bash command: sudo ls /root",
            "--vault",
            "/tmp/sandbox-e2e-sudo-test",
        ])
        .output()
        .expect("failed to launch amplifier-android-sandbox binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!(
        "stdout:
{stdout}"
    );
    println!(
        "stderr:
{stderr}"
    );

    let combined = format!("{stdout}{stderr}").to_lowercase();

    assert!(
        combined.contains("toybox") || combined.contains("blocked") || combined.contains("denied"),
        "expected output to contain 'toybox', 'blocked', or 'denied', got:
{stdout}
stderr:
{stderr}"
    );
}
