pub mod pool;
pub mod protocol;
pub mod schedule;
pub mod worker;

pub use pool::{WorkerPool, path_to_module};
pub use schedule::{DistMode, WorkUnit, partition, partition_with_hooks};
pub use worker::WorkerProcess;

use std::path::Path;

#[must_use]
pub fn resolve_python(root: &Path) -> String {
    let venv = root.join(".venv/bin/python3");
    if venv.exists() {
        venv.to_string_lossy().into_owned()
    } else {
        "python3".to_owned()
    }
}

/// Verify the resolved Python binary meets the `requires-python` specifier
/// from the project's `pyproject.toml`. Returns `Ok(())` when no specifier
/// is present or the version satisfies it.
#[expect(clippy::missing_errors_doc)]
pub fn check_python_version(python: &str, root: &Path) -> anyhow::Result<()> {
    let pyproject = root.join("pyproject.toml");
    let contents = std::fs::read_to_string(pyproject).ok();
    let specifier = contents.as_deref().and_then(tryke_config::requires_python);
    let Some(specifier) = specifier else {
        return Ok(());
    };
    let min = parse_min_version(&specifier)?;
    let output = std::process::Command::new(python)
        .args([
            "-c",
            "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run {python}: {e}"))?;
    let actual = parse_version(String::from_utf8_lossy(&output.stdout).trim())?;
    if actual < min {
        anyhow::bail!(
            "tryke requires Python {specifier} (from pyproject.toml), \
             found Python {}.{} ({python})",
            actual.0,
            actual.1
        );
    }
    Ok(())
}

fn parse_min_version(specifier: &str) -> anyhow::Result<(u32, u32)> {
    let version_part = specifier
        .strip_prefix(">=")
        .ok_or_else(|| anyhow::anyhow!("unsupported requires-python specifier: {specifier}"))?;
    parse_version(version_part)
}

fn parse_version(s: &str) -> anyhow::Result<(u32, u32)> {
    // Accept X.Y and X.Y.Z... — only the first two components are
    // significant for Python's requires-python floor.
    let mut parts = s.splitn(3, '.');
    let major = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid version: {s}"))?;
    let minor = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid version: {s}"))?;
    Ok((major.parse()?, minor.parse()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_min_version_ge() {
        assert_eq!(parse_min_version(">=3.12").unwrap(), (3, 12));
    }

    #[test]
    fn parse_min_version_rejects_unsupported() {
        assert!(parse_min_version("~=3.12").is_err());
    }

    #[test]
    fn parse_version_valid() {
        assert_eq!(parse_version("3.12").unwrap(), (3, 12));
        assert_eq!(parse_version("3.9").unwrap(), (3, 9));
    }

    #[test]
    fn parse_version_accepts_patch_component() {
        // PEP 440 allows requires-python with patch-level versions.
        // Home Assistant declares ">=3.14.2" — we only care about major.minor.
        assert_eq!(parse_version("3.14.2").unwrap(), (3, 14));
        assert_eq!(parse_min_version(">=3.14.2").unwrap(), (3, 14));
    }

    #[test]
    fn check_python_version_no_pyproject() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_python_version("python3", dir.path()).is_ok());
    }

    #[test]
    fn check_python_version_no_requires_python() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"app\"\n",
        )
        .unwrap();
        assert!(check_python_version("python3", dir.path()).is_ok());
    }

    #[test]
    fn check_python_version_satisfied() {
        let dir = tempfile::tempdir().unwrap();
        // Python 3.11+ is guaranteed in this env; use a low bar
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.9\"\n",
        )
        .unwrap();
        assert!(check_python_version("python3", dir.path()).is_ok());
    }

    #[test]
    fn check_python_version_too_old() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nrequires-python = \">=4.0\"\n",
        )
        .unwrap();
        let err = check_python_version("python3", dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(">=4.0"),
            "error should mention specifier: {msg}"
        );
        assert!(
            msg.contains("from pyproject.toml"),
            "error should mention source: {msg}"
        );
    }
}
