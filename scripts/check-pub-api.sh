#!/usr/bin/env bash
# scripts/check-pub-api.sh
# Fails (exit 1) if any public item in any publishable crate lacks a /// doc comment.
# Used by CI before publish.
set -euo pipefail
shopt -s globstar nullglob

CRATES=(
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
  amplifier-module-tool-skills
  amplifier-module-tool-delegate
  amplifier-module-orchestrator-loop-streaming
  amplifier-module-agent-runtime
  amplifier-module-session-store
  amplifier-agent-foundation
)

crate_path() {
  case "$1" in
    amplifier-agent-foundation) echo "amplifier-agent-foundation/src" ;;
    *) echo "crates/$1/src" ;;
  esac
}

fail=0
for crate in "${CRATES[@]}"; do
  src="$(crate_path "$crate")"
  if [[ ! -d "$src" ]]; then
    echo "::warning:: skipping $crate (no src dir at $src)"
    continue
  fi
  echo "=== $crate ==="
  RUSTDOCFLAGS="-D missing-docs" cargo doc --no-deps -p "$crate" --lib --quiet 2>/tmp/doc-err.log || {
    echo "DOC FAIL: $crate"; cat /tmp/doc-err.log; fail=1; continue
  }
  rs_files=("$src"/**/*.rs)
  if [[ ${#rs_files[@]} -eq 0 ]]; then
    echo "::warning:: no .rs files found in $src"
    continue
  fi
  awk '
    /^[[:space:]]*\/\/\// { has_doc = 1; next }
    /^[[:space:]]*#\[/    { next }
    /^[[:space:]]*pub (fn|struct|enum|trait|mod|const|static|type) / {
      if (!has_doc) { print FILENAME ":" NR ": undocumented: " $0; bad = 1 }
      has_doc = 0; next
    }
    { has_doc = 0 }
    END { exit bad }
  ' "${rs_files[@]}" 2>/dev/null || { echo "AWK PROBE FAIL: $crate"; fail=1; }
done
if [[ $fail -ne 0 ]]; then
  echo; echo "FAIL: undocumented public API in one or more crates"; exit 1
fi
echo; echo "PASS: all public API documented across ${#CRATES[@]} crates"
