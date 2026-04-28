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
    /// Tests within a describe() group go to one worker
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

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[command(flatten)]
    pub verbose: LogVerbosity<WarnLevel>,

    /// Disable the terminal's native graphical progress bar (Ghostty,
    /// Windows Terminal, ConEmu, WezTerm, iTerm2 OSC 9;4)
    #[arg(long = "no-progress", global = true)]
    pub no_progress: bool,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum ReporterFormat {
    Text,
    Json,
    Dot,
    Junit,
    Llm,
    Next,
    Sugar,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Collect and run tests.
    Test {
        /// File paths or file:line specs to restrict collection
        paths: Vec<String>,
        /// Exclude files/directories from discovery (overrides pyproject config)
        #[arg(short = 'e', long = "exclude")]
        exclude: Vec<String>,
        /// Include files/directories even if excluded by `pyproject.toml`
        #[arg(short = 'i', long = "include")]
        include: Vec<String>,
        /// Collect tests without running them
        #[arg(long)]
        collect_only: bool,
        /// Filter expression (e.g. "math and not slow")
        #[arg(short = 'k', long = "filter")]
        filter: Option<String>,
        /// Tag/marker filter expression (e.g. "slow and not network")
        #[arg(short = 'm', long = "markers")]
        markers: Option<String>,
        /// Reporter format to use for output
        #[arg(long = "reporter", default_value = "text")]
        reporter: ReporterFormat,
        /// Project root used for discovery and execution
        #[arg(long)]
        root: Option<PathBuf>,
        /// Use an already-running server on the optional port
        #[arg(long, default_missing_value = "2337", num_args = 0..=1, require_equals = false)]
        port: Option<u16>,
        /// Run only tests affected by files changed since HEAD (requires git)
        #[arg(long, conflicts_with = "changed_first")]
        changed: bool,
        /// Run changed tests first, then all remaining tests (requires git)
        #[arg(long, conflicts_with = "changed")]
        changed_first: bool,
        /// Base branch for --changed or --changed-first (e.g. "main"). Uses merge-base diff.
        #[arg(long)]
        base_branch: Option<String>,
        /// Stop after first failure
        #[arg(short = 'x', long = "fail-fast")]
        fail_fast: bool,
        /// Stop after N failures
        #[arg(long)]
        maxfail: Option<usize>,
        /// Number of worker processes (default: min(test_count, cpu_count))
        #[arg(short = 'j', long = "workers")]
        workers: Option<usize>,
        /// How tests are distributed across workers
        #[arg(long, default_value = "test")]
        dist: Dist,
    },
    /// Watch files and rerun affected tests.
    Watch {
        /// Exclude files/directories from discovery (overrides pyproject config)
        #[arg(short = 'e', long = "exclude")]
        exclude: Vec<String>,
        /// Include files/directories even if excluded by `pyproject.toml`
        #[arg(short = 'i', long = "include")]
        include: Vec<String>,
        /// Filter expression (e.g. "math and not slow")
        #[arg(short = 'k', long = "filter")]
        filter: Option<String>,
        /// Tag/marker filter expression (e.g. "slow and not network")
        #[arg(short = 'm', long = "markers")]
        markers: Option<String>,
        /// Reporter format to use for output
        #[arg(long = "reporter", default_value = "text")]
        reporter: ReporterFormat,
        /// Project root used for discovery and execution
        #[arg(long)]
        root: Option<PathBuf>,
        /// Stop after first failure
        #[arg(short = 'x', long = "fail-fast")]
        fail_fast: bool,
        /// Stop after N failures
        #[arg(long)]
        maxfail: Option<usize>,
        /// Number of worker processes (default: cpu_count)
        #[arg(short = 'j', long = "workers")]
        workers: Option<usize>,
        /// How tests are distributed across workers
        #[arg(long, default_value = "test")]
        dist: Dist,
        /// Rerun the full test set on every change instead of just affected tests
        #[arg(short = 'a', long = "all")]
        all: bool,
    },
    /// Start a persistent worker server.
    Server {
        /// Port for the server
        #[arg(long, default_value = "2337")]
        port: u16,
        /// Project root used for discovery and execution
        #[arg(long)]
        root: Option<PathBuf>,
        /// Exclude files/directories from discovery (overrides pyproject config)
        #[arg(short = 'e', long = "exclude")]
        exclude: Vec<String>,
        /// Include files/directories even if excluded by `pyproject.toml`
        #[arg(short = 'i', long = "include")]
        include: Vec<String>,
    },
    /// Print the import dependency graph for the project.
    Graph {
        /// Project root used for discovery and execution
        #[arg(long)]
        root: Option<PathBuf>,
        /// Exclude files/directories from discovery (overrides pyproject config)
        #[arg(short = 'e', long = "exclude")]
        exclude: Vec<String>,
        /// Include files/directories even if excluded by `pyproject.toml`
        #[arg(short = 'i', long = "include")]
        include: Vec<String>,
        /// Show only files that have dependents or dependencies (skip isolated files)
        #[arg(long)]
        connected_only: bool,
        /// Show only files affected by files changed since HEAD (requires git)
        #[arg(long)]
        changed: bool,
        /// Base branch for --changed (e.g. "main"). Uses merge-base diff.
        #[arg(long)]
        base_branch: Option<String>,
        /// Print the fixture (`@fixture` + `Depends()`) dependency graph
        /// instead of the import graph.
        #[arg(long, conflicts_with_all = ["connected_only", "changed", "base_branch"])]
        fixtures: bool,
    },
}
