//! Shared dev helpers used across tryke crates' test modules.
//!
//! Lives outside the production crates so the same logic isn't copy-pasted
//! into five different test modules and quietly drift apart (the venv
//! layout and Windows-vs-Unix fallback are easy to get wrong in only one
//! place).

use std::path::PathBuf;

use tryke_config::TrykeConfig;

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
/// Uses the same environment discovery as production.
#[must_use]
pub fn python_bin() -> String {
    TrykeConfig::discover(&workspace_root()).python()
}
