#!/bin/bash
# SessionStart hook for Claude Code on the web.
# Installs cargo-nextest (the project's canonical test runner per CLAUDE.md)
# and pre-warms the Rust + Python test suites.
set -euo pipefail

cd "${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel)}"

# Install cargo-nextest if missing. `cargo install --locked` is idempotent:
# it skips reinstall when the binary is already at the requested version.
if ! command -v cargo-nextest >/dev/null 2>&1; then
  cargo install cargo-nextest --locked
fi

cargo nextest run --workspace --all-features
