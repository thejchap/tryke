use std::{
    env, fs,
    path::{Component, Path, PathBuf},
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

impl DiscoveryConfig {
    /// Resolves configured source roots relative to `root`.
    #[must_use]
    pub fn src_roots(&self, root: &Path) -> Vec<PathBuf> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if self.src.is_empty() {
            return vec![root];
        }
        self.src
            .iter()
            .map(|entry| {
                let joined = root.join(entry);
                joined.canonicalize().unwrap_or(joined)
            })
            .collect()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigOverrides {
    pub python: Option<String>,
    pub cache_dir: Option<PathBuf>,
    pub exclude: Vec<String>,
    pub include: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct EnvironmentConfig {
    virtual_env: Option<PathBuf>,
    conda_child: Option<PathBuf>,
    conda_base: Option<PathBuf>,
}

impl EnvironmentConfig {
    fn from_env() -> Self {
        let virtual_env = non_empty_env_path("VIRTUAL_ENV");
        let conda_prefix = non_empty_env_path("CONDA_PREFIX");
        let (conda_child, conda_base) = match conda_prefix {
            Some(prefix) if conda_environment_is_base(&prefix) => (None, Some(prefix)),
            Some(prefix) => (Some(prefix), None),
            None => (None, None),
        };
        Self {
            virtual_env,
            conda_child,
            conda_base,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConfigValue<T> {
    value: T,
    relative_to: PathBuf,
}

impl<T> ConfigValue<T> {
    fn new(value: T, relative_to: &Path) -> Self {
        Self {
            value,
            relative_to: relative_to.to_path_buf(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrykeConfig {
    pub discovery: DiscoveryConfig,
    project_root: PathBuf,
    python: Option<ConfigValue<String>>,
    cache_dir: Option<ConfigValue<PathBuf>>,
    environment: EnvironmentConfig,
}

impl TrykeConfig {
    #[must_use]
    pub fn discover(start: &Path) -> Self {
        Self::load(start, ConfigOverrides::default())
    }

    #[must_use]
    pub fn load(start: &Path, overrides: ConfigOverrides) -> Self {
        let project_root = resolve_project_root(start);
        let config_root = find_config_root(&project_root);

        let file = config_root
            .as_deref()
            .and_then(|root| fs::read_to_string(root.join("pyproject.toml")).ok())
            .and_then(|contents| parse_toml(&contents))
            .unwrap_or_default();

        let value_root = config_root.as_deref().unwrap_or(&project_root);

        let exclude = if overrides.exclude.is_empty() {
            let includes = overrides
                .include
                .iter()
                .collect::<std::collections::HashSet<_>>();
            file.exclude
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| !includes.contains(entry))
                .collect()
        } else {
            overrides.exclude
        };

        let python = overrides
            .python
            .map(|value| ConfigValue::new(value, &project_root))
            .or_else(|| file.python.map(|value| ConfigValue::new(value, value_root)));

        let cache_dir = overrides
            .cache_dir
            .map(|value| ConfigValue::new(value, &project_root))
            .or_else(|| {
                file.cache_dir
                    .map(|value| ConfigValue::new(value, value_root))
            });

        Self {
            discovery: DiscoveryConfig {
                exclude,
                src: file.src.unwrap_or_else(|| vec![".".into()]),
            },
            project_root,
            python,
            cache_dir,
            environment: EnvironmentConfig::from_env(),
        }
    }

    /// Resolves the Python interpreter used to spawn worker processes.
    ///
    /// Precedence follows ty's environment discovery: CLI override,
    /// `[tool.tryke] python`, `VIRTUAL_ENV`, a child Conda environment,
    /// project `.venv`, a base Conda environment, then the platform's
    /// default Python command from `PATH`.
    #[must_use]
    pub fn python(&self) -> String {
        if let Some(value) = self.python.as_ref() {
            return resolve_python_value(&value.value, &value.relative_to);
        }
        if let Some(prefix) = self.environment.virtual_env.as_deref() {
            return python_in_environment(prefix).to_string_lossy().into_owned();
        }
        if let Some(prefix) = self.environment.conda_child.as_deref() {
            return python_in_environment(prefix).to_string_lossy().into_owned();
        }
        if !self.project_root.as_os_str().is_empty() {
            let venv = self.project_root.join(".venv");
            if venv.is_dir() {
                return python_in_environment(&venv).to_string_lossy().into_owned();
            }
        }
        if let Some(prefix) = self.environment.conda_base.as_deref() {
            return python_in_environment(prefix).to_string_lossy().into_owned();
        }
        default_python().to_owned()
    }

    /// Resolves the persistent discovery cache directory.
    #[must_use]
    pub fn cache_dir(&self) -> Option<PathBuf> {
        self.cache_dir
            .as_ref()
            .map(|value| anchor_path(&value.value, &value.relative_to))
    }

    #[must_use]
    pub fn src_roots(&self) -> Vec<PathBuf> {
        self.discovery.src_roots(&self.project_root)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.project_root
    }
}

fn default_python() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
}

#[must_use]
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join("pyproject.toml").exists())
        .map(Path::to_path_buf)
}

#[must_use]
pub fn resolve_project_root(start: &Path) -> PathBuf {
    let start = absolute(start);
    let root = find_project_root(&start).unwrap_or(start);
    root.canonicalize().unwrap_or(root)
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
            parse_toml(&contents).is_some()
        })
        .map(Path::to_path_buf)
}

fn resolve_python_value(value: &str, base: &Path) -> String {
    let path = Path::new(value);
    let has_separator = value.contains('/') || value.contains('\\');
    let has_prefix = matches!(path.components().next(), Some(Component::Prefix(_)));
    let is_environment = base.join(path).is_dir();
    if !has_separator && !path.is_absolute() && !path.has_root() && !has_prefix && !is_environment {
        return value.to_owned();
    }
    let resolved = anchor_path(path, base);
    if resolved.is_dir() {
        python_in_environment(&resolved)
            .to_string_lossy()
            .into_owned()
    } else {
        resolved.to_string_lossy().into_owned()
    }
}

fn anchor_path(value: &Path, base: &Path) -> PathBuf {
    let has_prefix = matches!(value.components().next(), Some(Component::Prefix(_)));
    if value.is_absolute() || value.has_root() || has_prefix {
        return value.to_path_buf();
    }
    base.join(value)
}

fn python_in_environment(environment: &Path) -> PathBuf {
    if cfg!(windows) {
        let venv_python = environment.join("Scripts/python.exe");
        if venv_python.exists() {
            venv_python
        } else {
            environment.join("python.exe")
        }
    } else {
        environment.join("bin/python")
    }
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn conda_environment_is_base(prefix: &Path) -> bool {
    if non_empty_env_path("_CONDA_ROOT").as_deref() == Some(prefix) {
        return true;
    }
    let Some(name) = env::var_os("CONDA_DEFAULT_ENV").filter(|value| !value.is_empty()) else {
        return false;
    };
    if name == "base" || name == "root" {
        return true;
    }
    prefix.file_name().is_none_or(|file_name| file_name != name)
}

/// Default `RUST_LOG`-style filter directive when `RUST_LOG` is unset.
///
/// `env_logger` honors `RUST_LOG` natively for fine-grained per-module
/// filtering; this only computes the fallback used when the user hasn't
/// set it. Precedence: `TRYKE_LOG` env > CLI flag.
///
/// `TRYKE_LOG` is a bare level name (`off`/`error`/`warn`/`info`/`debug`/
/// `trace`); `RUST_LOG`'s `tryke=info,hyper=warn` syntax is intentionally
/// not supported here — power users with that need just set `RUST_LOG`.
#[must_use]
pub fn rust_log_default(tryke_log_env: Option<&str>, cli: log::LevelFilter) -> log::LevelFilter {
    parse_level(tryke_log_env).unwrap_or(cli)
}

/// Level forwarded to spawned python workers via `TRYKE_LOG`.
///
/// Precedence: `TRYKE_LOG` env (if set) > CLI flag (only when explicitly
/// more verbose than `Warn` — i.e., the user passed at least one `-v`).
/// Returns `Off` when the worker should stay silent; callers should not
/// set the env var on the child in that case so the worker preserves its
/// "no chatter unless asked" default.
///
/// `RUST_LOG` is deliberately not consulted: it's a rust-specific
/// convention from `env_logger`, and silently translating its
/// per-module filter syntax into a python log level is a footgun.
/// `TRYKE_LOG` is the cross-language umbrella.
#[must_use]
pub fn worker_log_level(tryke_log_env: Option<&str>, cli: log::LevelFilter) -> log::LevelFilter {
    if let Some(level) = parse_level(tryke_log_env) {
        return level;
    }
    if cli > log::LevelFilter::Warn {
        cli
    } else {
        log::LevelFilter::Off
    }
}

fn parse_level(s: Option<&str>) -> Option<log::LevelFilter> {
    s.and_then(|v| v.trim().parse::<log::LevelFilter>().ok())
}

fn parse_toml(contents: &str) -> Option<RawTrykeConfig> {
    toml::from_str::<PyprojectToml>(contents).ok()?.tool?.tryke
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectToml {
    tool: Option<PyprojectTool>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectTool {
    tryke: Option<RawTrykeConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct RawTrykeConfig {
    exclude: Option<Vec<String>>,
    src: Option<Vec<String>>,
    python: Option<String>,
    cache_dir: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn tempdir() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn load_without_environment(start: &Path, overrides: ConfigOverrides) -> TrykeConfig {
        let mut config = TrykeConfig::load(start, overrides);
        config.environment = EnvironmentConfig::default();
        config
    }

    #[test]
    fn parses_tryke_tool_section() {
        let config = parse_toml("[tool.tryke]\nexclude = [\"generated/suites\", \"generated\"]\n")
            .expect("tryke config");
        assert_eq!(
            config.exclude,
            Some(vec!["generated/suites".into(), "generated".into()])
        );
    }

    #[test]
    fn parses_src_roots() {
        let config = parse_toml("[tool.tryke]\nsrc = [\".\", \"python\"]\n").expect("some");
        assert_eq!(config.src, Some(vec![".".into(), "python".into()]));
    }

    #[test]
    fn loads_default_src_when_unset() {
        let dir = tempdir();
        fs::write(dir.path().join("pyproject.toml"), "[tool.tryke]\n").expect("write pyproject");
        let config = load_without_environment(dir.path(), ConfigOverrides::default());
        assert_eq!(config.discovery.src, vec!["."]);
    }

    #[test]
    fn src_roots_resolve_relative_to_project_root() {
        let dir = tempdir();
        let python = dir.path().join("python");
        fs::create_dir(&python).expect("create python source root");
        let config = DiscoveryConfig {
            exclude: Vec::new(),
            src: vec![".".into(), "python".into()],
        };

        assert_eq!(
            config.src_roots(dir.path()),
            vec![
                dir.path().canonicalize().expect("canonical project root"),
                python.canonicalize().expect("canonical python source root"),
            ]
        );
    }

    #[test]
    fn empty_src_roots_fall_back_to_project_root() {
        let dir = tempdir();
        let config = DiscoveryConfig {
            exclude: Vec::new(),
            src: Vec::new(),
        };

        assert_eq!(
            config.src_roots(dir.path()),
            vec![dir.path().canonicalize().expect("canonical project root")]
        );
    }

    #[test]
    fn parses_python_path() {
        let config = parse_toml("[tool.tryke]\npython = \"/usr/bin/python3.13\"\n").expect("some");
        assert_eq!(config.python.as_deref(), Some("/usr/bin/python3.13"));
    }

    #[test]
    fn parses_cache_dir_path() {
        let config = parse_toml("[tool.tryke]\ncache_dir = \".cache/tryke\"\n").expect("some");
        assert_eq!(config.cache_dir.as_deref(), Some(Path::new(".cache/tryke")));
    }

    #[test]
    fn returns_none_when_no_tryke_section_exists() {
        let config = parse_toml("[project]\nname = \"app\"\n");
        assert!(config.is_none());
    }

    #[test]
    fn finds_project_root_from_child_directory() {
        let dir = tempdir();
        let child = dir.path().join("src");
        fs::create_dir(&child).expect("create child");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject");

        assert_eq!(find_project_root(&child), Some(dir.path().to_path_buf()));
        assert_eq!(
            resolve_project_root(&child),
            dir.path().canonicalize().expect("canonical project root")
        );
    }

    #[test]
    fn project_root_falls_back_to_start() {
        let dir = tempdir();
        assert_eq!(find_project_root(dir.path()), None);
        assert_eq!(
            resolve_project_root(dir.path()),
            dir.path().canonicalize().expect("canonical start")
        );
    }

    #[test]
    fn loads_empty_values_when_no_tryke_config_exists() {
        let dir = tempdir();
        let nested = dir.path().join("packages/app/src");
        fs::create_dir_all(&nested).expect("create nested");
        fs::write(
            dir.path().join("packages/app/pyproject.toml"),
            "[project]\nname = \"app\"\n",
        )
        .expect("write nested pyproject");

        let config = load_without_environment(&nested, ConfigOverrides::default());
        assert_eq!(config.discovery, DiscoveryConfig::default());
        assert_eq!(config.python, None);
        assert_eq!(config.cache_dir, None);
    }

    #[test]
    fn skips_intermediate_pyproject_without_tryke_section() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"generated/suites\"]\n",
        )
        .expect("write root pyproject");
        let nested = dir.path().join("packages/app/src");
        fs::create_dir_all(&nested).expect("create nested");
        fs::write(
            dir.path().join("packages/app/pyproject.toml"),
            "[project]\nname = \"app\"\n",
        )
        .expect("write nested pyproject");

        let config = load_without_environment(&nested, ConfigOverrides::default());
        assert_eq!(config.discovery.exclude, vec!["generated/suites"]);
    }

    #[test]
    fn nearest_tryke_config_wins() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"generated/suites\"]\n",
        )
        .expect("write root pyproject");
        let nested = dir.path().join("packages/app/src");
        fs::create_dir_all(&nested).expect("create nested");
        fs::write(
            dir.path().join("packages/app/pyproject.toml"),
            "[tool.tryke]\nexclude = [\"generated\"]\n",
        )
        .expect("write nested pyproject");

        let config = load_without_environment(&nested, ConfigOverrides::default());
        assert_eq!(config.discovery.exclude, vec!["generated"]);
    }

    #[test]
    fn cli_excludes_override_toml_excludes() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"generated\"]\n",
        )
        .expect("write pyproject");
        let config = load_without_environment(
            dir.path(),
            ConfigOverrides {
                exclude: vec!["build".into()],
                ..ConfigOverrides::default()
            },
        );
        assert_eq!(config.discovery.exclude, vec!["build"]);
    }

    #[test]
    fn cli_includes_remove_toml_excludes() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"generated\", \"build\"]\n",
        )
        .expect("write pyproject");
        let config = load_without_environment(
            dir.path(),
            ConfigOverrides {
                include: vec!["generated".into()],
                ..ConfigOverrides::default()
            },
        );
        assert_eq!(config.discovery.exclude, vec!["build"]);
    }

    #[test]
    fn python_resolves_toml_path_against_config_root() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\npython = \".venv/bin/python3\"\n",
        )
        .expect("write pyproject");
        let nested = dir.path().join("subdir");
        fs::create_dir_all(&nested).expect("create nested");
        let config = load_without_environment(&nested, ConfigOverrides::default());
        let expected = dir
            .path()
            .canonicalize()
            .expect("canonical config root")
            .join(".venv/bin/python3")
            .to_string_lossy()
            .into_owned();
        assert_eq!(config.python(), expected);
    }

    #[test]
    fn python_leaves_absolute_toml_path_unchanged() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\npython = \"/usr/bin/python3.13\"\n",
        )
        .expect("write pyproject");
        let config = load_without_environment(dir.path(), ConfigOverrides::default());
        assert_eq!(config.python(), "/usr/bin/python3.13");
    }

    #[test]
    fn cache_dir_resolves_toml_path_against_config_root() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\ncache_dir = \".cache/tryke\"\n",
        )
        .expect("write pyproject");
        let nested = dir.path().join("subdir");
        fs::create_dir_all(&nested).expect("create nested");
        let config = load_without_environment(&nested, ConfigOverrides::default());
        assert_eq!(
            config.cache_dir(),
            Some(
                dir.path()
                    .canonicalize()
                    .expect("canonical config root")
                    .join(".cache/tryke")
            )
        );
    }

    #[test]
    fn cache_dir_leaves_absolute_toml_path_unchanged() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\ncache_dir = \"/tmp/tryke-cache\"\n",
        )
        .expect("write pyproject");
        let config = load_without_environment(dir.path(), ConfigOverrides::default());
        assert_eq!(config.cache_dir(), Some(PathBuf::from("/tmp/tryke-cache")));
    }

    #[test]
    fn python_prefers_cli_override() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\npython = \"/from/config\"\n",
        )
        .expect("write pyproject");
        let config = load_without_environment(
            dir.path(),
            ConfigOverrides {
                python: Some("/from/cli".into()),
                ..ConfigOverrides::default()
            },
        );
        assert_eq!(config.python(), "/from/cli");
    }

    #[test]
    fn cli_python_path_resolves_against_project_root() {
        let dir = tempdir();
        let config = load_without_environment(
            dir.path(),
            ConfigOverrides {
                python: Some(".venv/bin/python".into()),
                ..ConfigOverrides::default()
            },
        );
        assert_eq!(
            config.python(),
            dir.path()
                .canonicalize()
                .expect("canonical project root")
                .join(".venv/bin/python")
                .to_string_lossy()
        );
    }

    #[test]
    fn python_uses_virtual_env_before_project_venv() {
        let dir = tempdir();
        let active = dir.path().join("active");
        let project = dir.path().join("project");
        fs::create_dir_all(project.join(".venv")).expect("create project venv");
        let config = TrykeConfig {
            project_root: project,
            environment: EnvironmentConfig {
                virtual_env: Some(active.clone()),
                conda_child: None,
                conda_base: None,
            },
            ..TrykeConfig::default()
        };

        assert_eq!(
            config.python(),
            python_in_environment(&active).to_string_lossy()
        );
    }

    #[test]
    fn python_uses_child_conda_before_project_venv() {
        let dir = tempdir();
        let conda = dir.path().join("conda");
        let project = dir.path().join("project");
        fs::create_dir_all(project.join(".venv")).expect("create project venv");
        let config = TrykeConfig {
            project_root: project,
            environment: EnvironmentConfig {
                virtual_env: None,
                conda_child: Some(conda.clone()),
                conda_base: None,
            },
            ..TrykeConfig::default()
        };

        assert_eq!(
            config.python(),
            python_in_environment(&conda).to_string_lossy()
        );
    }

    #[test]
    fn python_uses_project_venv_before_base_conda() {
        let dir = tempdir();
        let conda = dir.path().join("conda");
        let venv = dir.path().join(".venv");
        fs::create_dir(&venv).expect("create project venv");
        let config = TrykeConfig {
            project_root: dir.path().to_path_buf(),
            environment: EnvironmentConfig {
                virtual_env: None,
                conda_child: None,
                conda_base: Some(conda),
            },
            ..TrykeConfig::default()
        };

        assert_eq!(
            config.python(),
            python_in_environment(&venv).to_string_lossy()
        );
    }

    #[test]
    fn python_discovers_project_venv() {
        let dir = tempdir();
        let venv = dir.path().join(".venv");
        fs::create_dir(&venv).expect("create project venv");
        let config = TrykeConfig {
            project_root: dir.path().to_path_buf(),
            ..TrykeConfig::default()
        };

        assert_eq!(
            config.python(),
            python_in_environment(&venv).to_string_lossy()
        );
    }

    #[test]
    fn python_accepts_environment_directory() {
        let dir = tempdir();
        let venv = dir.path().join(".venv");
        fs::create_dir(&venv).expect("create project venv");
        let config = TrykeConfig {
            project_root: dir.path().to_path_buf(),
            python: Some(ConfigValue::new(".venv".into(), dir.path())),
            ..TrykeConfig::default()
        };

        assert_eq!(
            config.python(),
            python_in_environment(&venv).to_string_lossy()
        );
    }

    #[cfg(unix)]
    #[test]
    fn python_preserves_virtualenv_interpreter_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempdir();
        let managed = dir.path().join("managed/python");
        let venv_python = dir.path().join(".venv/bin/python");
        fs::create_dir_all(managed.parent().expect("managed parent")).expect("create managed");
        fs::create_dir_all(venv_python.parent().expect("venv parent")).expect("create venv");
        fs::write(&managed, "").expect("write managed python");
        symlink(&managed, &venv_python).expect("link venv python");
        let config = TrykeConfig {
            project_root: dir.path().to_path_buf(),
            python: Some(ConfigValue::new(".venv/bin/python".into(), dir.path())),
            ..TrykeConfig::default()
        };

        assert_eq!(config.python(), venv_python.to_string_lossy());
        assert_ne!(
            Path::new(&config.python())
                .canonicalize()
                .expect("canonical python"),
            PathBuf::from(config.python())
        );
    }

    #[test]
    fn python_defaults_to_platform_command() {
        let config = TrykeConfig::default();
        let expected = if cfg!(windows) { "python" } else { "python3" };
        assert_eq!(config.python(), expected);
    }

    #[test]
    fn cache_dir_prefers_cli_override() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\ncache_dir = \"/from/config\"\n",
        )
        .expect("write pyproject");
        let config = load_without_environment(
            dir.path(),
            ConfigOverrides {
                cache_dir: Some(PathBuf::from("/from/cli")),
                ..ConfigOverrides::default()
            },
        );
        assert_eq!(config.cache_dir(), Some(PathBuf::from("/from/cli")));
    }

    #[test]
    fn cache_dir_defaults_to_none() {
        let config = TrykeConfig::default();
        assert_eq!(config.cache_dir(), None);
    }

    #[test]
    fn python_leaves_bare_executable_name_unchanged() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\npython = \"python3\"\n",
        )
        .expect("write pyproject");
        let config = load_without_environment(dir.path(), ConfigOverrides::default());
        assert_eq!(config.python(), "python3");
    }

    #[cfg(windows)]
    #[test]
    fn python_leaves_drive_relative_path_unchanged() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\npython = 'C:foo\\python.exe'\n",
        )
        .expect("write pyproject");
        let config = load_without_environment(dir.path(), ConfigOverrides::default());
        assert_eq!(config.python(), "C:foo\\python.exe");
    }

    #[test]
    fn rust_log_default_uses_tryke_log_env_when_set() {
        let level = rust_log_default(Some("debug"), log::LevelFilter::Warn);
        assert_eq!(level, log::LevelFilter::Debug);
    }

    #[test]
    fn rust_log_default_falls_back_to_cli_when_env_unset() {
        let level = rust_log_default(None, log::LevelFilter::Info);
        assert_eq!(level, log::LevelFilter::Info);
    }

    #[test]
    fn rust_log_default_falls_back_to_cli_when_env_unparseable() {
        // Garbage values fall through rather than blowing up — RUST_LOG
        // could carry per-module filters we don't try to interpret here.
        let level = rust_log_default(Some("tryke=info,hyper=warn"), log::LevelFilter::Warn);
        assert_eq!(level, log::LevelFilter::Warn);
    }

    #[test]
    fn worker_log_level_uses_tryke_log_env() {
        let level = worker_log_level(Some("INFO"), log::LevelFilter::Warn);
        assert_eq!(level, log::LevelFilter::Info);
    }

    #[test]
    fn worker_log_level_propagates_explicit_verbose_flag() {
        let level = worker_log_level(None, log::LevelFilter::Debug);
        assert_eq!(level, log::LevelFilter::Debug);
    }

    #[test]
    fn worker_log_level_stays_off_at_default_warn() {
        // No env, no explicit `-v` → workers stay silent; preserves the
        // pre-existing "no chatter unless asked" default for python.
        let level = worker_log_level(None, log::LevelFilter::Warn);
        assert_eq!(level, log::LevelFilter::Off);
    }

    #[test]
    fn worker_log_level_stays_off_when_quiet() {
        // `-q` (Error) is even less verbose than the Warn default, so the
        // worker definitely shouldn't be lit up.
        let level = worker_log_level(None, log::LevelFilter::Error);
        assert_eq!(level, log::LevelFilter::Off);
    }

    #[test]
    fn worker_log_level_env_wins_over_cli() {
        // User explicitly set TRYKE_LOG, even though they also passed `-q`
        // — env intent dominates the flag.
        let level = worker_log_level(Some("info"), log::LevelFilter::Error);
        assert_eq!(level, log::LevelFilter::Info);
    }
}
