//! Shared dev helpers used across tryke crates' test modules.
//!
//! Lives outside the production crates so the same logic isn't copy-pasted
//! into five different test modules and quietly drift apart (the venv
//! layout and Windows-vs-Unix fallback are easy to get wrong in only one
//! place).

use std::path::PathBuf;

use tryke_config::{TrykeConfig, resolve_python};

/// Path to the workspace root, derived from this crate's manifest dir.
///
/// Anchoring on this crate's `CARGO_MANIFEST_DIR` (rather than the
/// caller's) makes the helper callable from any test module in the
/// workspace without each crate computing its own relative offset.
#[must_use]
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Returns a Python interpreter suitable for spawning workers in tests.
///
/// Prefers the workspace's uv-managed `.venv` if it exists (so tests pick
/// up the project's `requires-python` interpreter even when no venv is
/// active in the shell), otherwise delegates to `tryke_config::resolve_python`
/// so the bare-name fallback always matches whatever production picks.
#[must_use]
pub fn python_bin() -> String {
    let workspace = workspace_root();
    let venv = if cfg!(windows) {
        workspace.join(".venv/Scripts/python.exe")
    } else {
        workspace.join(".venv/bin/python3")
    };
    if venv.exists() {
        venv.to_string_lossy().into_owned()
    } else {
        resolve_python(None, &TrykeConfig::default())
    }
}
