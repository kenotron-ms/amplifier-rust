#!/usr/bin/env bash
# scripts/dryrun-all.sh — runs cargo publish --dry-run for every publishable crate
# in topological tier order. CI uses this as a publish gate.
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
    cargo publish --dry-run -p "$c"
  done
}

run_tier "TIER 1" "${TIER1[@]}"
run_tier "TIER 2" "${TIER2[@]}"
echo "ALL DRY-RUNS PASS"
