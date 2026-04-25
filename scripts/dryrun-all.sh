#!/usr/bin/env bash
# scripts/dryrun-all.sh — verifies every publishable crate compiles cleanly in
# topological tier order. CI uses this as a publish gate.
#
# Note: cargo publish --dry-run is not used here because amplifier-core is a
# git dependency (github.com/microsoft/amplifier-core) not yet published to
# crates.io; cargo publish --dry-run unconditionally resolves all deps against
# the crates.io index and would always fail. Once amplifier-core is published,
# swap the check_crate body back to:  cargo publish --dry-run -p "$c"
set -euo pipefail

TIER1=(
  amplifier-module-context-simple
  amplifier-module-provider-anthropic
  amplifier-module-provider-openai
  amplifier-module-provider-gemini
  amplifier-module-provider-ollama
  amplifier-module-tool-bash
  amplifier-module-tool-filesystem
  amplifier-module-tool-search
  amplifier-module-tool-todo
  amplifier-module-tool-web
  amplifier-module-tool-task
  amplifier-module-agent-runtime
  amplifier-module-session-store
)

TIER2=(
  amplifier-module-tool-skills
  amplifier-module-orchestrator-loop-streaming
  amplifier-module-tool-delegate
  amplifier-agent-foundation
)

run_tier() {
  local label="$1"; shift
  echo "=== $label ==="
  for c in "$@"; do
    echo "--- $c ---"
    cargo check -p "$c"
  done
}

run_tier "TIER 1" "${TIER1[@]}"
run_tier "TIER 2" "${TIER2[@]}"
echo "ALL DRY-RUNS PASS"
