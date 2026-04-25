use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryConfig {
    pub exclude: Vec<String>,
    /// Source roots for absolute-import resolution. `tryke.worker` is
    /// tried as `<root>/tryke/worker.py` (then `.../__init__.py`) under
    /// each root in order, matching how `sys.path` layers multiple
    /// package roots. Defaults to `["."]` — the project root.
    pub src: Vec<String>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            src: vec![".".into()],
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrykeConfig {
    pub discovery: DiscoveryConfig,
}

impl TrykeConfig {
    #[must_use]
    pub fn from_toml_str(contents: &str) -> Option<Self> {
        let raw = toml::from_str::<PyprojectToml>(contents).ok()?;
        raw.tool.and_then(|tool| {
            tool.tryke.or(tool.trike).map(|config| Self {
                discovery: DiscoveryConfig {
                    exclude: config.exclude.unwrap_or_default(),
                    src: config.src.unwrap_or_else(|| vec![".".into()]),
                },
            })
        })
    }
}

#[must_use]
pub fn find_config_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| {
            let pyproject = dir.join("pyproject.toml");
            let Ok(contents) = fs::read_to_string(pyproject) else {
                return false;
            };
            TrykeConfig::from_toml_str(&contents).is_some()
        })
        .map(Path::to_path_buf)
}

#[must_use]
pub fn load_effective_config(start: &Path) -> TrykeConfig {
    let Some(root) = find_config_root(start) else {
        return TrykeConfig::default();
    };
    let Ok(contents) = fs::read_to_string(root.join("pyproject.toml")) else {
        return TrykeConfig::default();
    };
    TrykeConfig::from_toml_str(&contents).unwrap_or_default()
}

#[must_use]
pub fn requires_python(contents: &str) -> Option<String> {
    let raw = toml::from_str::<PyprojectToml>(contents).ok()?;
    raw.project.and_then(|p| p.requires_python)
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectToml {
    project: Option<PyprojectProject>,
    tool: Option<PyprojectTool>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectProject {
    #[serde(rename = "requires-python")]
    requires_python: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectTool {
    tryke: Option<RawTrykeConfig>,
    trike: Option<RawTrykeConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct RawTrykeConfig {
    exclude: Option<Vec<String>>,
    src: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn tempdir() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn parses_tryke_tool_section() {
        let config = TrykeConfig::from_toml_str(
            "[tool.tryke]\nexclude = [\"benchmarks/suites\", \"generated\"]\n",
        );
        assert_eq!(
            config,
            Some(TrykeConfig {
                discovery: DiscoveryConfig {
                    exclude: vec!["benchmarks/suites".into(), "generated".into()],
                    src: vec![".".into()],
                },
            })
        );
    }

    #[test]
    fn parses_legacy_trike_alias() {
        let config = TrykeConfig::from_toml_str("[tool.trike]\nexclude = [\"generated\"]\n");
        assert_eq!(
            config,
            Some(TrykeConfig {
                discovery: DiscoveryConfig {
                    exclude: vec!["generated".into()],
                    src: vec![".".into()],
                },
            })
        );
    }

    #[test]
    fn parses_src_roots() {
        let config =
            TrykeConfig::from_toml_str("[tool.tryke]\nsrc = [\".\", \"python\"]\n").expect("some");
        assert_eq!(config.discovery.src, vec![".", "python"]);
    }

    #[test]
    fn src_defaults_to_project_root_when_unset() {
        let config = TrykeConfig::from_toml_str("[tool.tryke]\n").expect("some");
        assert_eq!(config.discovery.src, vec!["."]);
    }

    #[test]
    fn returns_none_when_no_tryke_section_exists() {
        let config = TrykeConfig::from_toml_str("[project]\nname = \"app\"\n");
        assert_eq!(config, None);
    }

    #[test]
    fn returns_default_when_no_tryke_config_exists() {
        let dir = tempdir();
        let nested = dir.path().join("packages/app/src");
        fs::create_dir_all(&nested).expect("create nested");
        fs::write(
            dir.path().join("packages/app/pyproject.toml"),
            "[project]\nname = \"app\"\n",
        )
        .expect("write nested pyproject");

        let config = load_effective_config(&nested);
        assert_eq!(config, TrykeConfig::default());
    }

    #[test]
    fn skips_intermediate_pyproject_without_tryke_section() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"benchmarks/suites\"]\n",
        )
        .expect("write root pyproject");
        let nested = dir.path().join("packages/app/src");
        fs::create_dir_all(&nested).expect("create nested");
        fs::write(
            dir.path().join("packages/app/pyproject.toml"),
            "[project]\nname = \"app\"\n",
        )
        .expect("write nested pyproject");

        let config = load_effective_config(&nested);
        assert_eq!(config.discovery.exclude, vec!["benchmarks/suites"]);
    }

    #[test]
    fn nearest_tryke_config_wins() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"benchmarks/suites\"]\n",
        )
        .expect("write root pyproject");
        let nested = dir.path().join("packages/app/src");
        fs::create_dir_all(&nested).expect("create nested");
        fs::write(
            dir.path().join("packages/app/pyproject.toml"),
            "[tool.tryke]\nexclude = [\"generated\"]\n",
        )
        .expect("write nested pyproject");

        let config = load_effective_config(&nested);
        assert_eq!(config.discovery.exclude, vec!["generated"]);
    }

    #[test]
    fn requires_python_extracts_specifier() {
        let spec = requires_python("[project]\nrequires-python = \">=3.12\"\n");
        assert_eq!(spec.as_deref(), Some(">=3.12"));
    }

    #[test]
    fn requires_python_returns_none_when_missing() {
        let spec = requires_python("[project]\nname = \"app\"\n");
        assert_eq!(spec, None);
    }

    #[test]
    fn requires_python_returns_none_without_project_table() {
        let spec = requires_python("[tool.tryke]\nexclude = []\n");
        assert_eq!(spec, None);
    }
}
