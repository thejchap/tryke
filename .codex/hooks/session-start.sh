#!/bin/bash
# SessionStart hook for Codex (wired via .codex/hooks.json).
# Installs cargo-nextest (the project's canonical test runner per CLAUDE.md)
# if it is not already available.
set -euo pipefail

cd "${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel)}"

# Install cargo-nextest if missing. `cargo install --locked` is idempotent:
# it skips reinstall when the binary is already at the requested version.
if ! command -v cargo-nextest >/dev/null 2>&1; then
  cargo install cargo-nextest --locked
fi
