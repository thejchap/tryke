use std::{
    fs,
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
pub struct TrykeConfig {
    pub discovery: DiscoveryConfig,
    /// Path to the Python interpreter used to spawn worker processes.
    /// `None` means fall back to `python` on Windows / `python3` on Unix
    /// (per `default_python()`).
    pub python: Option<String>,
    /// Directory for tryke's persistent discovery cache.
    ///
    /// `None` means use the default `<project-root>/.tryke/cache` path.
    pub cache_dir: Option<PathBuf>,
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
                python: config.python,
                cache_dir: config.cache_dir,
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
    let mut config = TrykeConfig::from_toml_str(&contents).unwrap_or_default();
    // Mirror `execvp` / `CreateProcess` semantics: a value containing a
    // path separator is treated as a filesystem path; anything else is a
    // bare command name to look up via `PATH`. For paths that are
    // genuinely relative (no root, no drive prefix), anchor them to the
    // directory containing `pyproject.toml` so configs like
    // `python = ".venv/bin/python3"` work regardless of the cwd from
    // which tryke is invoked. Windows drive-relative values like
    // `C:foo\python.exe` carry a `Component::Prefix` but no root and are
    // *not* `is_absolute()` — leaving them to the OS's per-drive cwd
    // resolution is closer to user intent than rewriting them onto the
    // config root.
    if let Some(py) = config.python.as_deref() {
        let has_separator = py.contains('/') || py.contains('\\');
        let path = Path::new(py);
        let has_prefix = matches!(path.components().next(), Some(Component::Prefix(_)));
        if has_separator && !path.is_absolute() && !path.has_root() && !has_prefix {
            config.python = Some(root.join(path).to_string_lossy().into_owned());
        }
    }
    if let Some(cache_dir) = config.cache_dir.as_deref() {
        let has_prefix = matches!(cache_dir.components().next(), Some(Component::Prefix(_)));
        if !cache_dir.is_absolute() && !cache_dir.has_root() && !has_prefix {
            config.cache_dir = Some(root.join(cache_dir));
        }
    }
    config
}

/// Default Python binary name when neither a CLI flag nor a config value
/// is provided. Windows venvs ship `python.exe` (no `python3.exe` shim),
/// while Linux and macOS conventionally expose `python3`.
fn default_python() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

/// Resolve the Python interpreter for spawning worker processes.
///
/// Precedence: CLI override > `[tool.tryke] python` in `pyproject.toml` >
/// `python` (Windows) / `python3` (elsewhere) on `PATH`. Environment
/// management (venv activation, `uv run`, etc.) is the user's
/// responsibility — tryke does not introspect or validate the chosen
/// interpreter.
#[must_use]
pub fn resolve_python(cli_override: Option<&str>, config: &TrykeConfig) -> String {
    cli_override
        .map(str::to_owned)
        .or_else(|| config.python.clone())
        .unwrap_or_else(|| default_python().to_owned())
}

/// Resolve the discovery cache directory.
///
/// Precedence: CLI override > `[tool.tryke] cache_dir` in `pyproject.toml` >
/// `None`, which tells discovery to use its default `<project-root>/.tryke/cache`.
#[must_use]
pub fn resolve_cache_dir(cli_override: Option<&Path>, config: &TrykeConfig) -> Option<PathBuf> {
    cli_override
        .map(Path::to_path_buf)
        .or_else(|| config.cache_dir.clone())
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

#[derive(Debug, Default, Deserialize)]
struct PyprojectToml {
    tool: Option<PyprojectTool>,
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

    #[test]
    fn parses_tryke_tool_section() {
        let config = TrykeConfig::from_toml_str(
            "[tool.tryke]\nexclude = [\"generated/suites\", \"generated\"]\n",
        );
        assert_eq!(
            config,
            Some(TrykeConfig {
                discovery: DiscoveryConfig {
                    exclude: vec!["generated/suites".into(), "generated".into()],
                    src: vec![".".into()],
                },
                python: None,
                cache_dir: None,
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
                python: None,
                cache_dir: None,
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
        let config = TrykeConfig::from_toml_str("[tool.tryke]\npython = \"/usr/bin/python3.13\"\n")
            .expect("some");
        assert_eq!(config.python.as_deref(), Some("/usr/bin/python3.13"));
    }

    #[test]
    fn python_defaults_to_none() {
        let config = TrykeConfig::from_toml_str("[tool.tryke]\n").expect("some");
        assert_eq!(config.python, None);
    }

    #[test]
    fn parses_cache_dir_path() {
        let config = TrykeConfig::from_toml_str("[tool.tryke]\ncache_dir = \".cache/tryke\"\n")
            .expect("some");
        assert_eq!(config.cache_dir.as_deref(), Some(Path::new(".cache/tryke")));
    }

    #[test]
    fn cache_dir_defaults_to_none() {
        let config = TrykeConfig::from_toml_str("[tool.tryke]\n").expect("some");
        assert_eq!(config.cache_dir, None);
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

        let config = load_effective_config(&nested);
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

        let config = load_effective_config(&nested);
        assert_eq!(config.discovery.exclude, vec!["generated"]);
    }

    #[test]
    fn load_effective_config_resolves_relative_python_against_config_root() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\npython = \".venv/bin/python3\"\n",
        )
        .expect("write pyproject");
        let nested = dir.path().join("subdir");
        fs::create_dir_all(&nested).expect("create nested");
        let config = load_effective_config(&nested);
        let expected = dir
            .path()
            .join(".venv/bin/python3")
            .to_string_lossy()
            .into_owned();
        assert_eq!(config.python, Some(expected));
    }

    #[test]
    fn load_effective_config_leaves_absolute_python_unchanged() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\npython = \"/usr/bin/python3.13\"\n",
        )
        .expect("write pyproject");
        let config = load_effective_config(dir.path());
        assert_eq!(config.python.as_deref(), Some("/usr/bin/python3.13"));
    }

    #[test]
    fn load_effective_config_resolves_relative_cache_dir_against_config_root() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\ncache_dir = \".cache/tryke\"\n",
        )
        .expect("write pyproject");
        let nested = dir.path().join("subdir");
        fs::create_dir_all(&nested).expect("create nested");
        let config = load_effective_config(&nested);
        assert_eq!(config.cache_dir, Some(dir.path().join(".cache/tryke")));
    }

    #[test]
    fn load_effective_config_leaves_absolute_cache_dir_unchanged() {
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\ncache_dir = \"/tmp/tryke-cache\"\n",
        )
        .expect("write pyproject");
        let config = load_effective_config(dir.path());
        assert_eq!(config.cache_dir, Some(PathBuf::from("/tmp/tryke-cache")));
    }

    #[test]
    fn resolve_python_prefers_cli_override() {
        let config = TrykeConfig {
            python: Some("/from/config".into()),
            ..TrykeConfig::default()
        };
        assert_eq!(resolve_python(Some("/from/cli"), &config), "/from/cli");
    }

    #[test]
    fn resolve_python_falls_back_to_config() {
        let config = TrykeConfig {
            python: Some("/from/config".into()),
            ..TrykeConfig::default()
        };
        assert_eq!(resolve_python(None, &config), "/from/config");
    }

    #[test]
    fn resolve_python_defaults_to_platform_default() {
        let config = TrykeConfig::default();
        let expected = if cfg!(windows) { "python" } else { "python3" };
        assert_eq!(resolve_python(None, &config), expected);
    }

    #[test]
    fn resolve_cache_dir_prefers_cli_override() {
        let config = TrykeConfig {
            cache_dir: Some(PathBuf::from("/from/config")),
            ..TrykeConfig::default()
        };
        assert_eq!(
            resolve_cache_dir(Some(Path::new("/from/cli")), &config),
            Some(PathBuf::from("/from/cli"))
        );
    }

    #[test]
    fn resolve_cache_dir_falls_back_to_config() {
        let config = TrykeConfig {
            cache_dir: Some(PathBuf::from("/from/config")),
            ..TrykeConfig::default()
        };
        assert_eq!(
            resolve_cache_dir(None, &config),
            Some(PathBuf::from("/from/config"))
        );
    }

    #[test]
    fn resolve_cache_dir_defaults_to_none() {
        let config = TrykeConfig::default();
        assert_eq!(resolve_cache_dir(None, &config), None);
    }

    #[test]
    fn load_effective_config_leaves_bare_executable_name_unchanged() {
        // `python = "python3"` should resolve via PATH, not be rewritten
        // to `<config-root>/python3`.
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\npython = \"python3\"\n",
        )
        .expect("write pyproject");
        let config = load_effective_config(dir.path());
        assert_eq!(config.python.as_deref(), Some("python3"));
    }

    #[cfg(windows)]
    #[test]
    fn load_effective_config_leaves_drive_relative_python_unchanged() {
        // `C:foo\python.exe` is drive-relative on Windows — it has a
        // `Component::Prefix` but no root and is not `is_absolute()`.
        // Rewriting it onto the config root would mangle the value.
        // Written via a TOML literal string (single quotes) so backslashes
        // are passed through verbatim, no escape processing.
        let dir = tempdir();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\npython = 'C:foo\\python.exe'\n",
        )
        .expect("write pyproject");
        let config = load_effective_config(dir.path());
        assert_eq!(config.python.as_deref(), Some("C:foo\\python.exe"));
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
