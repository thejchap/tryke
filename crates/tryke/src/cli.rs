use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use clap_verbosity_flag::{Verbosity as LogVerbosity, WarnLevel};

/// How tests are distributed across workers.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Dist {
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
/// a pluggable reporter. It can also run as a long-lived server that keeps
/// workers warm between file changes for sub-second feedback in editors.
///
/// Run `tryke <command> --help` to see detailed help for a subcommand.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[command(flatten)]
    pub verbose: LogVerbosity<WarnLevel>,

    /// Disable the terminal's native graphical progress bar.
    ///
    /// By default tryke emits OSC 9;4 progress sequences, which terminals
    /// like Ghostty, WezTerm, iTerm2, Windows Terminal, and ConEmu render as
    /// a native progress indicator (taskbar badge, tab badge, etc.). Pass
    /// this flag in CI or in terminals that mis-render the sequence.
    #[arg(long = "no-progress", global = true)]
    pub no_progress: bool,
}

/// Reporter format used to render test results.
#[derive(Clone, Debug, ValueEnum)]
pub enum ReporterFormat {
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

#[derive(Debug, Subcommand)]
pub enum Commands {
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
    Test {
        /// File paths or `file:line` specs to restrict collection.
        ///
        /// Each path may be a file, a directory, or `file.py:LINE` to target
        /// the test defined at that line. Directory paths recurse into all
        /// `.py` files under them.
        #[arg(conflicts_with = "watch")]
        paths: Vec<String>,

        /// Exclude files or directories from discovery.
        ///
        /// Overrides the `[tool.tryke] exclude` list in `pyproject.toml`.
        /// May be repeated.
        #[arg(short = 'e', long = "exclude")]
        exclude: Vec<String>,

        /// Include files or directories even if excluded by `pyproject.toml`.
        ///
        /// Useful for opting a single subtree back into discovery without
        /// rewriting the project-wide exclude list. May be repeated.
        #[arg(short = 'i', long = "include")]
        include: Vec<String>,

        /// Collect tests without running them.
        ///
        /// Prints the discovered test list and exits. Useful for verifying
        /// that filters select the tests you expect.
        #[arg(long, conflicts_with = "watch")]
        collect_only: bool,

        /// Filter tests by name expression.
        ///
        /// Supports substring matching with boolean operators (`and`, `or`,
        /// `not`) and parentheses, matched against the full test name
        /// including any `describe()` group prefix.
        ///
        /// Examples: `-k "math"`, `-k "math and not slow"`, `-k "(parse or
        /// lex) and not regression"`.
        #[arg(short = 'k', long = "filter")]
        filter: Option<String>,

        /// Filter tests by tag expression.
        ///
        /// Matches against the `tags=[...]` argument on the `@test`
        /// decorator. Same boolean syntax as `-k`.
        ///
        /// Examples: `-m "slow"`, `-m "fast and not network"`.
        #[arg(short = 'm', long = "markers")]
        markers: Option<String>,

        /// Reporter format for test output.
        #[arg(long = "reporter", default_value = "text")]
        reporter: ReporterFormat,

        /// Project root used for discovery and execution.
        ///
        /// Defaults to the current working directory. Discovery, the import
        /// graph, and `pyproject.toml` resolution are all anchored here.
        #[arg(long)]
        root: Option<PathBuf>,

        /// Run against an already-running `tryke server` instead of spawning
        /// fresh workers.
        ///
        /// Pass `--port` alone to use the default `2337`, or `--port 9000`
        /// to target a specific port. The server keeps workers pre-warmed
        /// and the import graph cached, so this is significantly faster for
        /// repeated runs.
        #[arg(long, default_missing_value = "2337", num_args = 0..=1, require_equals = false, conflicts_with = "watch")]
        port: Option<u16>,

        /// Run only tests affected by uncommitted changes.
        ///
        /// Uses `git diff` to find changed `.py` files, then walks the
        /// import graph forward to find every test that transitively
        /// depends on a changed module. Combine with `--base-branch` to
        /// diff against a branch instead of the working tree.
        #[arg(long, conflicts_with_all = ["changed_first", "watch"])]
        changed: bool,

        /// Run changed tests first, then the remaining tests.
        ///
        /// Same affected-set computation as `--changed`, but unaffected
        /// tests are appended to the run rather than skipped. Gives fast
        /// feedback on the diff while still verifying the full suite.
        #[arg(long, conflicts_with_all = ["changed", "watch"])]
        changed_first: bool,

        /// Base branch for `--changed` / `--changed-first` diff.
        ///
        /// Compares against `git merge-base <base> HEAD` instead of the
        /// working tree. Typical CI usage: `--changed --base-branch
        /// origin/main`.
        #[arg(long)]
        base_branch: Option<String>,

        /// Stop after the first failing test.
        #[arg(short = 'x', long = "fail-fast")]
        fail_fast: bool,

        /// Stop after `N` failures.
        ///
        /// Mutually informative with `--fail-fast` (which is `--maxfail 1`).
        #[arg(long)]
        maxfail: Option<usize>,

        /// Number of worker processes.
        ///
        /// Defaults to `min(test_count, cpu_count)`. Set to `1` to run
        /// tests in a single worker (useful when debugging concurrency
        /// issues).
        #[arg(short = 'j', long = "workers")]
        workers: Option<usize>,

        /// How tests are distributed across workers.
        #[arg(long, default_value = "test")]
        dist: Dist,

        /// Watch the project and rerun affected tests on each change.
        ///
        /// Enters an interactive loop: tryke watches all `.py` files
        /// (respecting `.gitignore`), and on each save it walks the import
        /// graph from the modified file forward to find affected tests,
        /// restarts the worker pool, and reruns just those tests. Press
        /// `q` to quit.
        #[arg(short = 'w', long = "watch")]
        watch: bool,

        /// In watch mode, rerun the full test set on every change.
        ///
        /// Disables affected-test computation; every save triggers a full
        /// run. Useful when the import graph is stale or for very small
        /// suites.
        #[arg(short = 'a', long = "all", requires = "watch")]
        all: bool,

        /// Path to the Python interpreter used to spawn worker processes.
        ///
        /// Overrides `[tool.tryke] python` in `pyproject.toml`. Defaults
        /// to `python` on Windows / `python3` on Unix from `PATH`. The
        /// interpreter is the user's responsibility — tryke does not
        /// validate it. Activate the appropriate venv (or use
        /// `uv run tryke ...`) and the default will pick it up.
        ///
        /// Relative `python` values in `pyproject.toml` (e.g.,
        /// `.venv/bin/python3`) resolve against the directory containing
        /// `pyproject.toml`, not the cwd. Bare names (`python3`, `pypy`)
        /// are looked up via `PATH`. See the `Configuration` guide for
        /// the full resolution rules.
        ///
        /// Not compatible with `--port`; configure the interpreter on
        /// the server instead.
        #[arg(long, conflicts_with = "port")]
        python: Option<String>,
    },

    /// Start a persistent worker server.
    ///
    /// Spawns and pre-warms the worker pool, runs initial discovery, and
    /// listens on `127.0.0.1:<port>` for JSON-RPC 2.0 requests. Clients
    /// connect with `tryke test --port` to run tests without paying the
    /// cold-start cost. The server also watches the filesystem and
    /// broadcasts `discover_complete` notifications when the test list
    /// changes.
    Server {
        /// Port for the server to listen on.
        #[arg(long, default_value = "2337")]
        port: u16,

        /// Project root used for discovery and execution.
        #[arg(long)]
        root: Option<PathBuf>,

        /// Exclude files or directories from discovery.
        #[arg(short = 'e', long = "exclude")]
        exclude: Vec<String>,

        /// Include files or directories even if excluded by `pyproject.toml`.
        #[arg(short = 'i', long = "include")]
        include: Vec<String>,

        /// Path to the Python interpreter used to spawn worker processes.
        ///
        /// Overrides `[tool.tryke] python` in `pyproject.toml`. Defaults
        /// to `python` on Windows / `python3` on Unix from `PATH`.
        /// Relative values in `pyproject.toml` resolve against the
        /// directory containing `pyproject.toml`; bare names go through
        /// `PATH`. See the `Configuration` guide for the full rules.
        #[arg(long)]
        python: Option<String>,
    },

    /// Print the import dependency graph for the project.
    ///
    /// Renders the static import graph that drives discovery, change
    /// detection, and watch mode. Defaults to printing reachable modules
    /// from the project root; pass `--changed` to see only the slice
    /// affected by recent edits, or `--fixtures` to inspect the fixture
    /// dependency graph (`@fixture` + `Depends()`) instead.
    Graph {
        /// Project root used for discovery.
        #[arg(long)]
        root: Option<PathBuf>,

        /// Exclude files or directories from discovery.
        #[arg(short = 'e', long = "exclude")]
        exclude: Vec<String>,

        /// Include files or directories even if excluded by `pyproject.toml`.
        #[arg(short = 'i', long = "include")]
        include: Vec<String>,

        /// Hide isolated nodes (files with no dependents and no dependencies).
        #[arg(long)]
        connected_only: bool,

        /// Show only the slice affected by changes since `HEAD`.
        ///
        /// Requires git. Combine with `--base-branch` to diff against a
        /// branch instead of the working tree.
        #[arg(long)]
        changed: bool,

        /// Base branch for `--changed`. Uses `git merge-base` diff.
        #[arg(long)]
        base_branch: Option<String>,

        /// Print the fixture dependency graph instead of the import graph.
        ///
        /// Renders the graph of `@fixture`-decorated functions and the
        /// `Depends()` edges between them, useful for debugging fixture
        /// resolution.
        #[arg(long, conflicts_with_all = ["connected_only", "changed", "base_branch"])]
        fixtures: bool,
    },
}
