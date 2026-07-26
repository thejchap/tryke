use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_verbosity_flag::{Verbosity as LogVerbosity, WarnLevel};
use tryke_config::TrykeOptions;
use tryke_reporter::ReporterKind;
use tryke_runner::default_worker_count;

/// How tests are distributed across workers.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub(crate) enum Dist {
    /// Each test is its own work unit (maximum parallelism)
    #[default]
    Test,
    /// All tests from a file go to one worker
    File,
    /// Tests within a `describe()` group go to one worker
    Group,
}

impl From<Dist> for tryke_runner::DistMode {
    fn from(d: Dist) -> Self {
        match d {
            Dist::Test => Self::Test,
            Dist::File => Self::File,
            Dist::Group => Self::Group,
        }
    }
}

/// A Rust-based Python test runner with a Jest-style API.
///
/// Tryke discovers tests by walking the project's import graph, runs them
/// across a pool of pre-warmed worker processes, and streams results through
/// a pluggable reporter. Its watch and server modes cache discovery while
/// starting fresh workers for each logical run.
///
/// Running `tryke` with no subcommand starts watch mode. Run `tryke
/// <command> --help` to see detailed help for a subcommand.
#[derive(Debug, Parser)]
#[command(version, about)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    #[command(flatten)]
    pub(crate) global: GlobalArgs,
}

#[derive(Debug, Args)]
pub(crate) struct GlobalArgs {
    #[command(flatten)]
    pub(crate) verbose: LogVerbosity<WarnLevel>,

    /// Disable the terminal's native graphical progress bar.
    ///
    /// By default tryke emits OSC 9;4 progress sequences, which terminals
    /// like Ghostty, WezTerm, iTerm2, Windows Terminal, and ConEmu render as
    /// a native progress indicator (taskbar badge, tab badge, etc.). Pass
    /// this flag in CI or in terminals that mis-render the sequence.
    #[arg(long = "no-progress", global = true)]
    pub(crate) no_progress: bool,

    /// Directory for tryke's persistent discovery cache.
    ///
    /// Overrides `[tool.tryke] cache_dir` in `pyproject.toml`. Defaults to
    /// `<project-root>/.tryke/cache`.
    #[arg(long = "cache-dir", global = true)]
    pub(crate) cache_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum SnapshotMode {
    /// Compare snapshots to persisted value
    Compare,

    /// Update snapshots to current value
    Update,
}

// impl SnapshotMode {
//     pub(crate) fn to_wire(&self) -> SnapshotModeWire {
//         match self {
//             SnapshotMode::Compare => SnapshotModeWire::Compare,
//             SnapshotMode::Update => SnapshotModeWire::Update,
//         }
//     }
// }

/// Reporter format used to render test results.
#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum ReporterFormat {
    /// Human-readable per-test output with assertion diagnostics
    Text,
    /// Newline-delimited JSON, one event per line
    Json,
    /// Graphviz DOT output (only meaningful for `tryke graph`)
    Dot,
    /// JUnit XML for CI systems that consume JUnit reports
    Junit,
    /// Compact format optimized for LLM context windows
    Llm,
    /// cargo-nextest-style status badges with a live progress bar
    Next,
    /// One-character-per-test compact dot reporter
    Sugar,
}

impl ReporterFormat {
    pub(crate) fn kind(&self) -> ReporterKind {
        match self {
            Self::Text => ReporterKind::Text,
            Self::Json => ReporterKind::Json,
            Self::Dot => ReporterKind::Dot,
            Self::Junit => ReporterKind::Junit,
            Self::Llm => ReporterKind::Llm,
            Self::Next => ReporterKind::Next,
            Self::Sugar => ReporterKind::Sugar,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    /// Collect and run tests.
    ///
    /// Discovers tests by walking the project's static import graph from the
    /// project root, then runs them across a worker pool. Filter with
    /// positional path arguments, `-k` (name expression), or `-m` (tag
    /// expression). Combine with `--changed` to run only tests affected by
    /// uncommitted changes, or `--watch` for an interactive rerun loop.
    ///
    /// Examples:
    ///
    /// ```bash
    /// tryke test
    /// tryke test tests/test_math.py
    /// tryke test tests/test_math.py:42
    /// tryke test -k "parse and not slow"
    /// tryke test --changed --base-branch origin/main
    /// tryke test --watch
    /// ```
    #[command(verbatim_doc_comment)]
    Test(TestArgs),

    /// Start a persistent worker server speaking JSON-RPC over stdio.
    ///
    /// Prepares the worker pool, runs initial discovery, then reads JSON-RPC
    /// 2.0 requests from stdin and writes responses and notifications to
    /// stdout. Messages are newline-delimited — one JSON object per line —
    /// not LSP's `Content-Length` framing. Each run uses fresh worker
    /// processes. Like a language server, editor plugins spawn `tryke server`
    /// as a child process and own its stdio; closing stdin shuts the server
    /// down. The server also watches the filesystem and emits
    /// `discover_complete` notifications when the test list changes.
    Server(ServerArgs),

    /// Remove tryke's persistent discovery cache.
    ///
    /// Deletes the default `<project-root>/.tryke/cache` directory. When
    /// `--cache-dir` or `[tool.tryke] cache_dir` points at a custom directory,
    /// only tryke-owned cache files inside that directory are removed.
    Clean(CleanArgs),

    /// Print the import dependency graph for the project.
    ///
    /// Renders the static import graph that drives discovery, change
    /// detection, and watch mode. Defaults to printing reachable modules
    /// from the project root; pass `--changed` to see only the slice
    /// affected by recent edits, or `--fixtures` to inspect the fixture
    /// dependency graph (`@fixture` + `Depends()`) instead.
    Graph(GraphArgs),
}

#[derive(Debug, Args)]
pub(crate) struct TestArgs {
    /// File paths or `file:line` specs to restrict collection.
    ///
    /// Each path may be a file, a directory, or `file.py:LINE` to target
    /// the test defined at that line. Directory paths recurse into all
    /// `.py` files under them.
    #[arg(conflicts_with = "watch")]
    pub(crate) paths: Vec<String>,

    /// Exclude files or directories from discovery.
    ///
    /// Overrides the `[tool.tryke] exclude` list in `pyproject.toml`.
    /// May be repeated.
    #[arg(short = 'e', long = "exclude")]
    pub(crate) exclude: Vec<String>,

    /// Include files or directories even if excluded by `pyproject.toml`.
    ///
    /// Useful for opting a single subtree back into discovery without
    /// rewriting the project-wide exclude list. May be repeated.
    #[arg(short = 'i', long = "include")]
    pub(crate) include: Vec<String>,

    /// Collect tests without running them.
    ///
    /// Prints the discovered test list and exits. Useful for verifying
    /// that filters select the tests you expect.
    #[arg(long, conflicts_with = "watch")]
    pub(crate) collect_only: bool,

    /// Filter tests by name expression.
    ///
    /// Supports substring matching with boolean operators (`and`, `or`,
    /// `not`) and parentheses, matched against the full test name
    /// including any `describe()` group prefix.
    ///
    /// Examples: `-k "math"`, `-k "math and not slow"`, `-k "(parse or
    /// lex) and not regression"`.
    #[arg(short = 'k', long = "filter")]
    pub(crate) filter: Option<String>,

    /// Filter tests by tag expression.
    ///
    /// Matches against the `tags=[...]` argument on the `@test`
    /// decorator. Same boolean syntax as `-k`.
    ///
    /// Examples: `-m "slow"`, `-m "fast and not network"`.
    #[arg(short = 'm', long = "markers")]
    pub(crate) markers: Option<String>,

    /// Reporter format for test output.
    #[arg(long = "reporter", default_value = "text")]
    pub(crate) reporter: ReporterFormat,

    /// Project root used for discovery and execution.
    ///
    /// Defaults to the current working directory. Discovery, the import
    /// graph, and `pyproject.toml` resolution are all anchored here.
    #[arg(long)]
    pub(crate) root: Option<PathBuf>,

    /// Run only tests affected by uncommitted changes.
    ///
    /// Uses `git diff` to find changed `.py` files, then walks the
    /// import graph forward to find every test that transitively
    /// depends on a changed module. Combine with `--base-branch` to
    /// diff against a branch instead of the working tree.
    #[arg(long, conflicts_with = "watch", group = "changed_selection")]
    pub(crate) changed: bool,

    /// Run changed tests first, then the remaining tests.
    ///
    /// Same affected-set computation as `--changed`, but unaffected
    /// tests are appended to the run rather than skipped. Gives fast
    /// feedback on the diff while still verifying the full suite.
    #[arg(long, conflicts_with = "watch", group = "changed_selection")]
    pub(crate) changed_first: bool,

    /// Base branch for `--changed` / `--changed-first` diff.
    ///
    /// Compares against `git merge-base <base> HEAD` instead of the
    /// working tree. Typical CI usage: `--changed --base-branch
    /// origin/main`.
    #[arg(long, requires = "changed_selection")]
    pub(crate) base_branch: Option<String>,

    /// Stop after the first failing test.
    #[arg(short = 'x', long = "fail-fast")]
    pub(crate) fail_fast: bool,

    /// Stop after `N` failures.
    ///
    /// Mutually informative with `--fail-fast` (which is `--maxfail 1`).
    #[arg(long)]
    pub(crate) maxfail: Option<usize>,

    /// Number of worker processes.
    ///
    /// Defaults to the CPU count. Set to `1` to run tests in a single
    /// worker, which is useful when debugging concurrency issues.
    #[arg(
        short = 'j',
        long = "workers",
        default_value_t = default_worker_count(),
        hide_default_value = true
    )]
    pub(crate) workers: usize,

    /// How tests are distributed across workers.
    #[arg(long, default_value = "test")]
    pub(crate) dist: Dist,

    /// Watch the project and rerun affected tests on each change.
    ///
    /// Enters an interactive loop: tryke watches all `.py` files
    /// (respecting `.gitignore`), and on each save it walks the import
    /// graph from the modified file forward to find affected tests,
    /// starts fresh workers, and reruns just those tests. Press `q` to
    /// quit, `enter` to run all tests, or `c` to clear results.
    #[arg(short = 'w', long = "watch")]
    pub(crate) watch: bool,

    /// In watch mode, rerun the full test set on every change.
    ///
    /// Disables affected-test computation; every save triggers a full
    /// run. Useful when the import graph is stale or for very small
    /// suites.
    #[arg(short = 'a', long = "all", requires = "watch")]
    pub(crate) all: bool,

    /// In watch mode, run tests immediately on watch startup.
    ///
    /// By default watch mode starts idle and waits for the first file
    /// change before running anything. Pass `--now` to kick off a full
    /// run on startup, the same way each subsequent change does.
    ///
    /// Requires `--watch`.
    #[arg(long = "now", requires = "watch")]
    pub(crate) now: bool,

    /// Path to the Python interpreter or environment used to spawn workers.
    ///
    /// Overrides `[tool.tryke] python` in `pyproject.toml`. When unset,
    /// Tryke checks `VIRTUAL_ENV`, Conda, and the project `.venv` before
    /// falling back to `python` on Windows / `python3` on Unix from
    /// `PATH`.
    ///
    /// Relative CLI paths resolve against the project root. Relative
    /// `python` values in `pyproject.toml` (e.g.,
    /// `.venv/bin/python3`) resolve against the directory containing
    /// `pyproject.toml`, not the cwd. Bare names (`python3`, `pypy`) are
    /// looked up via `PATH`. See the `Configuration` guide for the full
    /// resolution rules.
    #[arg(long)]
    pub(crate) python: Option<String>,
    // Snapshot mode.
    // #[arg(long, default_value = "compare")]
    // pub(crate) snapshot_mode: SnapshotMode,
}

impl TestArgs {
    fn default_watch() -> Self {
        Self {
            paths: Vec::new(),
            exclude: Vec::new(),
            include: Vec::new(),
            collect_only: false,
            filter: None,
            markers: None,
            reporter: ReporterFormat::Text,
            root: None,
            changed: false,
            changed_first: false,
            base_branch: None,
            fail_fast: false,
            maxfail: None,
            workers: default_worker_count(),
            dist: Dist::Test,
            watch: true,
            all: false,
            now: false,
            python: None,
            // snapshot_mode: SnapshotMode::Compare,
        }
    }

    pub(crate) fn project_options(&self, global: &GlobalArgs) -> TrykeOptions {
        project_options(
            self.python.as_deref(),
            global.cache_dir.as_deref(),
            &self.exclude,
            &self.include,
        )
    }
}

#[derive(Debug, Args)]
pub(crate) struct ServerArgs {
    /// Project root used for discovery and execution.
    #[arg(long)]
    pub(crate) root: Option<PathBuf>,

    /// Exclude files or directories from discovery.
    #[arg(short = 'e', long = "exclude")]
    pub(crate) exclude: Vec<String>,

    /// Include files or directories even if excluded by `pyproject.toml`.
    #[arg(short = 'i', long = "include")]
    pub(crate) include: Vec<String>,

    /// Path to the Python interpreter or environment used to spawn workers.
    ///
    /// Overrides `[tool.tryke] python` in `pyproject.toml`. When unset,
    /// Tryke checks `VIRTUAL_ENV`, Conda, and the project `.venv` before
    /// falling back to `python` on Windows / `python3` on Unix from `PATH`.
    /// Relative CLI paths resolve against the project root. Relative
    /// values in `pyproject.toml` resolve against the directory containing
    /// `pyproject.toml`; bare names go through `PATH`. See the
    /// `Configuration` guide for the full rules.
    #[arg(long)]
    pub(crate) python: Option<String>,

    /// Number of worker processes.
    ///
    /// Defaults to the CPU count. Set to `1` to run tests in a single
    /// worker, which is useful when debugging concurrency issues.
    #[arg(
        short = 'j',
        long = "workers",
        default_value_t = default_worker_count(),
        hide_default_value = true
    )]
    pub(crate) workers: usize,
}

impl ServerArgs {
    pub(crate) fn project_options(&self, global: &GlobalArgs) -> TrykeOptions {
        project_options(
            self.python.as_deref(),
            global.cache_dir.as_deref(),
            &self.exclude,
            &self.include,
        )
    }
}

#[derive(Debug, Args)]
pub(crate) struct CleanArgs {
    /// Project root used to resolve the default cache directory.
    #[arg(long)]
    pub(crate) root: Option<PathBuf>,
}

impl CleanArgs {
    pub(crate) fn project_options(&self, global: &GlobalArgs) -> TrykeOptions {
        project_options(None, global.cache_dir.as_deref(), &[], &[])
    }
}

#[derive(Debug, Args)]
pub(crate) struct GraphArgs {
    /// Project root used for discovery.
    #[arg(long)]
    pub(crate) root: Option<PathBuf>,

    /// Exclude files or directories from discovery.
    #[arg(short = 'e', long = "exclude")]
    pub(crate) exclude: Vec<String>,

    /// Include files or directories even if excluded by `pyproject.toml`.
    #[arg(short = 'i', long = "include")]
    pub(crate) include: Vec<String>,

    /// Hide isolated nodes (files with no dependents and no dependencies).
    #[arg(long)]
    pub(crate) connected_only: bool,

    /// Show only the slice affected by changes since `HEAD`.
    ///
    /// Requires git. Combine with `--base-branch` to diff against a
    /// branch instead of the working tree.
    #[arg(long)]
    pub(crate) changed: bool,

    /// Base branch for `--changed`. Uses `git merge-base` diff.
    #[arg(long, requires = "changed")]
    pub(crate) base_branch: Option<String>,

    /// Print the fixture dependency graph instead of the import graph.
    ///
    /// Renders the graph of `@fixture`-decorated functions and the
    /// `Depends()` edges between them, useful for debugging fixture
    /// resolution.
    #[arg(long, conflicts_with_all = ["connected_only", "changed", "base_branch"])]
    pub(crate) fixtures: bool,
}

impl GraphArgs {
    pub(crate) fn project_options(&self, global: &GlobalArgs) -> TrykeOptions {
        project_options(
            None,
            global.cache_dir.as_deref(),
            &self.exclude,
            &self.include,
        )
    }
}

fn project_options(
    python: Option<&str>,
    cache_dir: Option<&std::path::Path>,
    exclude: &[String],
    include: &[String],
) -> TrykeOptions {
    TrykeOptions {
        python: python.map(str::to_owned),
        cache_dir: cache_dir.map(std::path::Path::to_path_buf),
        exclude: (!exclude.is_empty()).then(|| exclude.to_vec()),
        include: (!include.is_empty()).then(|| include.to_vec()),
        src: None,
    }
}

impl Commands {
    #[must_use]
    pub(crate) fn default_watch() -> Self {
        Self::Test(TestArgs::default_watch())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser;
    use clap_verbosity_flag::log::LevelFilter;

    use super::*;

    #[test]
    fn parses_global_cache_dir_before_subcommand() {
        let cli = Cli::parse_from(["tryke", "--cache-dir", "/tmp/tryke-cache", "test"]);
        assert_eq!(
            cli.global.cache_dir.as_deref(),
            Some(Path::new("/tmp/tryke-cache"))
        );
    }

    #[test]
    fn parses_global_cache_dir_after_subcommand() {
        let cli = Cli::parse_from(["tryke", "test", "--cache-dir", "/tmp/tryke-cache"]);
        assert_eq!(
            cli.global.cache_dir.as_deref(),
            Some(Path::new("/tmp/tryke-cache"))
        );
    }

    #[test]
    fn parses_global_output_options() {
        let cli = Cli::parse_from(["tryke", "-vv", "test", "--no-progress"]);
        assert_eq!(cli.global.verbose.log_level_filter(), LevelFilter::Debug);
        assert!(cli.global.no_progress);
    }

    #[test]
    fn parses_default_worker_count() {
        let test = Cli::parse_from(["tryke", "test"]);
        assert!(matches!(
            test.command,
            Some(Commands::Test(TestArgs { workers, .. }))
                if workers == default_worker_count()
        ));

        let server = Cli::parse_from(["tryke", "server"]);
        assert!(matches!(
            server.command,
            Some(Commands::Server(ServerArgs { workers, .. }))
                if workers == default_worker_count()
        ));
    }

    #[test]
    fn parses_test_arguments() {
        let cli = Cli::parse_from([
            "tryke",
            "test",
            "tests/test_math.py",
            "-e",
            "generated",
            "-i",
            "generated/selected",
            "-k",
            "math",
            "-m",
            "fast",
            "--reporter",
            "json",
            "--root",
            "/tmp/project",
            "--changed-first",
            "--base-branch",
            "main",
            "--maxfail",
            "2",
            "-j",
            "4",
            "--dist",
            "file",
            "--python",
            "/usr/bin/python3",
            // "--snapshot-mode",
            // "update",
        ]);
        let args = match cli.command.as_ref() {
            Some(Commands::Test(args)) => Some(args),
            _ => None,
        };
        assert!(args.is_some(), "expected test command");
        let Some(args) = args else {
            return;
        };
        assert_eq!(args.paths, ["tests/test_math.py"]);
        assert_eq!(args.exclude, ["generated"]);
        assert_eq!(args.include, ["generated/selected"]);
        assert_eq!(args.filter.as_deref(), Some("math"));
        assert_eq!(args.markers.as_deref(), Some("fast"));
        assert!(matches!(args.reporter, ReporterFormat::Json));
        assert_eq!(args.root.as_deref(), Some(Path::new("/tmp/project")));
        assert!(args.changed_first);
        assert_eq!(args.base_branch.as_deref(), Some("main"));
        assert_eq!(args.maxfail, Some(2));
        assert_eq!(args.workers, 4);
        assert!(matches!(args.dist, Dist::File));
        assert_eq!(args.python.as_deref(), Some("/usr/bin/python3"));
        // assert!(matches!(args.snapshot_mode, SnapshotMode::Update));
    }

    #[test]
    fn test_argument_conflicts_are_preserved() {
        assert!(Cli::try_parse_from(["tryke", "test", "--watch", "--collect-only"]).is_err());
        assert!(Cli::try_parse_from(["tryke", "test", "--watch", "--changed"]).is_err());
        assert!(Cli::try_parse_from(["tryke", "test", "--changed", "--changed-first"]).is_err());
        assert!(Cli::try_parse_from(["tryke", "test", "--all"]).is_err());
        assert!(Cli::try_parse_from(["tryke", "test", "--now"]).is_err());
    }

    #[test]
    fn base_branch_requires_change_selection() {
        assert!(matches!(
            Cli::try_parse_from(["tryke", "test", "--base-branch", "main"]),
            Err(error) if error.kind() == clap::error::ErrorKind::MissingRequiredArgument
        ));
        assert!(matches!(
            Cli::try_parse_from(["tryke", "graph", "--base-branch", "main"]),
            Err(error) if error.kind() == clap::error::ErrorKind::MissingRequiredArgument
        ));

        assert!(
            Cli::try_parse_from(["tryke", "test", "--changed", "--base-branch", "main"]).is_ok()
        );
        assert!(
            Cli::try_parse_from(["tryke", "test", "--changed-first", "--base-branch", "main",])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from(["tryke", "graph", "--changed", "--base-branch", "main"]).is_ok()
        );
    }

    #[test]
    fn parses_server_clean_and_graph_arguments() {
        let server = Cli::parse_from(["tryke", "server", "--python", "pypy", "-j", "2"]);
        assert!(matches!(
            server.command,
            Some(Commands::Server(ServerArgs {
                python: Some(ref python),
                workers: 2,
                ..
            })) if python == "pypy"
        ));

        let clean = Cli::parse_from(["tryke", "clean", "--root", "/tmp/project"]);
        assert!(matches!(
            clean.command,
            Some(Commands::Clean(CleanArgs { root: Some(ref root) }))
                if root == Path::new("/tmp/project")
        ));

        let graph = Cli::parse_from(["tryke", "graph", "--changed", "--base-branch", "main"]);
        assert!(matches!(
            graph.command,
            Some(Commands::Graph(GraphArgs {
                changed: true,
                base_branch: Some(ref base_branch),
                ..
            })) if base_branch == "main"
        ));
    }

    #[test]
    fn default_command_is_bare_watch() {
        assert!(matches!(
            Commands::default_watch(),
            Commands::Test(TestArgs {
                watch: true,
                now: false,
                reporter: ReporterFormat::Text,
                ..
            })
        ));
    }
}
