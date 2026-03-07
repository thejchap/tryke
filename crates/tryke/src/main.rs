use std::{
    env,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use clap_verbosity_flag::{Verbosity as LogVerbosity, WarnLevel};
use log::{debug, warn};
use tokio_stream::StreamExt;
use tryke_discovery::Discoverer;
use tryke_reporter::{DotReporter, JSONReporter, JUnitReporter, Reporter, TextReporter, Verbosity};
use tryke_runner::{WorkerPool, resolve_python};
use tryke_types::{RunSummary, TestOutcome};

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[command(flatten)]
    verbose: LogVerbosity<WarnLevel>,
}

#[derive(Clone, Debug, ValueEnum)]
enum ReporterFormat {
    Text,
    Json,
    Dot,
    Junit,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Test {
        #[arg(long)]
        collect_only: bool,
        #[arg(long = "reporter", default_value = "text")]
        reporter: ReporterFormat,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long, default_missing_value = "2337", num_args = 0..=1, require_equals = false)]
        port: Option<u16>,
        /// Run only tests affected by files changed since HEAD (requires git)
        #[arg(long)]
        changed: bool,
    },
    Watch {
        #[arg(long = "reporter", default_value = "text")]
        reporter: ReporterFormat,
        #[arg(long)]
        root: Option<PathBuf>,
    },
    Server {
        #[arg(long, default_value = "2337")]
        port: u16,
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Print the import dependency graph for the project
    Graph {
        #[arg(long)]
        root: Option<PathBuf>,
        /// Show only files that have dependents or dependencies (skip isolated files)
        #[arg(long)]
        connected_only: bool,
    },
}

fn worker_pool_size() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
}

async fn run_test(reporter: &mut dyn Reporter, root: Option<&Path>) -> Result<()> {
    let start = Instant::now();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root_path = root.unwrap_or(&cwd);
    let tests = tryke_discovery::discover_from(root_path);
    reporter.on_run_start(&tests);

    let python = resolve_python(root_path);
    let pool = WorkerPool::new(worker_pool_size(), &python, root_path);
    let mut stream = pool.run(tests);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    while let Some(result) = stream.next().await {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
        }
        reporter.on_test_complete(&result);
    }

    reporter.on_run_complete(&RunSummary {
        passed,
        failed,
        skipped,
        duration: start.elapsed(),
    });

    pool.shutdown();
    Ok(())
}

fn run_collect_only(reporter: &mut dyn Reporter, root: Option<&Path>) -> Result<()> {
    let tests = match root {
        Some(r) => tryke_discovery::discover_from(r),
        None => tryke_discovery::discover(),
    };
    reporter.on_collect_complete(&tests);
    Ok(())
}

async fn report_cycle(
    reporter: &mut dyn Reporter,
    tests: Vec<tryke_types::TestItem>,
    pool: &WorkerPool,
) -> Result<()> {
    let start = Instant::now();
    reporter.on_run_start(&tests);

    let mut stream = pool.run(tests);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    while let Some(result) = stream.next().await {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
        }
        reporter.on_test_complete(&result);
    }

    reporter.on_run_complete(&RunSummary {
        passed,
        failed,
        skipped,
        duration: start.elapsed(),
    });

    Ok(())
}

async fn run_cycle(
    reporter: &mut dyn Reporter,
    discoverer: &mut Discoverer,
    pool: &WorkerPool,
) -> Result<()> {
    report_cycle(reporter, discoverer.rediscover(), pool).await
}

fn clear_if_tty() {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        let _ = clearscreen::clear();
    }
}

async fn run_watch(reporter: &mut dyn Reporter, root: Option<&Path>) -> Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new(root);

    let python = resolve_python(root);
    let pool = WorkerPool::new(worker_pool_size(), &python, root);

    clear_if_tty();
    run_cycle(reporter, &mut discoverer, &pool).await?;

    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let _debouncer = tryke_server::watcher::spawn_watcher(root, tx)?;

    for paths in &rx {
        let modules = discoverer.affected_modules(&paths);
        if !modules.is_empty() {
            pool.reload(modules).await;
        }
        discoverer.rediscover_changed(&paths);
        clear_if_tty();
        let tests = discoverer.tests_for_changed(&paths);
        report_cycle(reporter, tests, &pool).await?;
    }

    pool.shutdown();
    Ok(())
}

/// Collect changed files from `git diff --name-only HEAD` relative to `root`.
/// Returns `None` if git is unavailable or the command fails.
fn git_changed_files(root: &Path) -> Option<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let paths: Vec<PathBuf> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| root.join(l))
        .collect();
    Some(paths)
}

async fn run_changed_test(reporter: &mut dyn Reporter, root: Option<&Path>) -> Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root_path = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new(root_path);
    discoverer.rediscover();

    let tests = match git_changed_files(root_path) {
        Some(changed) if !changed.is_empty() => {
            debug!("--changed: {} git-changed files", changed.len());
            discoverer.tests_for_changed(&changed)
        }
        Some(_) => {
            warn!("--changed: no changed files found via git, running all tests");
            discoverer.tests()
        }
        None => {
            warn!("--changed: git unavailable or failed, running all tests");
            discoverer.tests()
        }
    };

    let start = Instant::now();
    reporter.on_run_start(&tests);
    let python = resolve_python(root_path);
    let pool = WorkerPool::new(worker_pool_size(), &python, root_path);
    let mut stream = pool.run(tests);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    while let Some(result) = stream.next().await {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
        }
        reporter.on_test_complete(&result);
    }
    reporter.on_run_complete(&RunSummary {
        passed,
        failed,
        skipped,
        duration: start.elapsed(),
    });
    pool.shutdown();
    Ok(())
}

fn run_graph(root: Option<&Path>, connected_only: bool) -> Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root_path = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new(root_path);
    discoverer.rediscover();

    let summary = discoverer.import_graph_summary();
    for entry in &summary {
        if connected_only && entry.imports.is_empty() && entry.imported_by.is_empty() {
            continue;
        }
        println!("{}", entry.file.display());
        if entry.imports.is_empty() {
            println!("  imports:     (none)");
        } else {
            let names: Vec<String> = entry
                .imports
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            println!("  imports:     {}", names.join(", "));
        }
        if entry.imported_by.is_empty() {
            println!("  imported by: (none)");
        } else {
            let names: Vec<String> = entry
                .imported_by
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            println!("  imported by: {}", names.join(", "));
        }
        println!();
    }
    Ok(())
}

fn build_reporter(format: &ReporterFormat, verbosity: Verbosity) -> Box<dyn Reporter> {
    match format {
        ReporterFormat::Text => Box::new(TextReporter::with_verbosity(verbosity)),
        ReporterFormat::Json => Box::new(JSONReporter::new()),
        ReporterFormat::Dot => Box::new(DotReporter::new()),
        ReporterFormat::Junit => Box::new(JUnitReporter::new()),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    env_logger::Builder::new()
        .filter_level(cli.verbose.log_level_filter())
        .init();
    debug!("{cli:?}");

    let verbosity = match cli.verbose.log_level() {
        Some(log::Level::Info) | Some(log::Level::Debug) | Some(log::Level::Trace) => {
            Verbosity::Verbose
        }
        Some(log::Level::Error) | None => Verbosity::Quiet,
        _ => Verbosity::Normal,
    };

    let rt = tokio::runtime::Runtime::new()?;
    match &cli.command {
        Commands::Test {
            collect_only,
            reporter,
            root,
            port,
            changed,
        } => {
            let mut rep = build_reporter(reporter, verbosity);
            let root_ref = root.as_deref();
            if let Some(p) = port {
                let root_path = root
                    .clone()
                    .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
                tryke_server::Client::new(*p).run(&root_path, &mut *rep)
            } else if *collect_only {
                run_collect_only(&mut *rep, root_ref)
            } else if *changed {
                rt.block_on(run_changed_test(&mut *rep, root_ref))
            } else {
                rt.block_on(run_test(&mut *rep, root_ref))
            }
        }
        Commands::Watch { reporter, root } => {
            let mut rep = build_reporter(reporter, verbosity);
            rt.block_on(run_watch(&mut *rep, root.as_deref()))
        }
        Commands::Server { port, root } => {
            let root_path = root
                .clone()
                .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let server = tryke_server::Server::new(*port, root_path);
            rt.block_on(server.run())
        }
        Commands::Graph {
            root,
            connected_only,
        } => run_graph(root.as_deref(), *connected_only),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap_verbosity_flag::log::LevelFilter;
    use tryke_reporter::{JSONReporter, TextReporter};
    use tryke_types::TestItem;

    use super::*;

    #[tokio::test]
    async fn test_command_text() {
        let mut reporter = TextReporter::new();
        assert!(run_test(&mut reporter, None).await.is_ok());
    }

    #[tokio::test]
    async fn test_command_json() {
        let mut reporter = JSONReporter::new();
        assert!(run_test(&mut reporter, None).await.is_ok());
    }

    #[tokio::test]
    async fn test_command_dot() {
        let mut reporter = DotReporter::new();
        assert!(run_test(&mut reporter, None).await.is_ok());
    }

    #[tokio::test]
    async fn test_command_junit() {
        let mut reporter = JUnitReporter::new();
        assert!(run_test(&mut reporter, None).await.is_ok());
    }

    #[test]
    fn test_verbose_flag_sets_debug_level() {
        let cli = Cli::try_parse_from(["tryke", "-vv", "test"]).unwrap();
        assert_eq!(cli.verbose.log_level_filter(), LevelFilter::Debug);
    }

    #[test]
    fn test_collect_only_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "--collect-only"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Test {
                collect_only: true,
                ..
            }
        ));
    }

    #[test]
    fn test_collect_only_text() {
        let mut reporter = TextReporter::with_writer(Vec::new());
        assert!(run_collect_only(&mut reporter, None).is_ok());
        let out = String::from_utf8_lossy(&reporter.into_writer()).into_owned();
        let tests = tryke_discovery::discover();
        for test in &tests {
            assert!(out.contains(&test.id()), "missing {} in output", test.id());
        }
        assert!(out.contains("tests collected."));
    }

    #[test]
    fn test_collect_only_json() {
        let mut reporter = JSONReporter::with_writer(Vec::new());
        assert!(run_collect_only(&mut reporter, None).is_ok());
        let buf = reporter.into_writer();
        let out = String::from_utf8_lossy(&buf);
        let val: serde_json::Value = serde_json::from_str(out.trim()).expect("valid json");
        assert_eq!(val["event"], "collect_complete");
        assert!(val["tests"].is_array());
    }

    #[test]
    fn test_root_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "--root", "/tmp"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Test {
                root: Some(p),
                ..
            } if p == &PathBuf::from("/tmp")
        ));
    }

    #[test]
    fn test_reporter_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "--reporter", "dot"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Test {
                reporter: ReporterFormat::Dot,
                ..
            }
        ));
    }

    #[test]
    fn test_verbose_flag_drives_verbose_output() {
        let cli = Cli::try_parse_from(["tryke", "test", "-v"]).unwrap();
        let verbosity = match cli.verbose.log_level() {
            Some(log::Level::Info) | Some(log::Level::Debug) | Some(log::Level::Trace) => {
                Verbosity::Verbose
            }
            Some(log::Level::Error) | None => Verbosity::Quiet,
            _ => Verbosity::Normal,
        };
        assert!(matches!(verbosity, Verbosity::Verbose));
    }

    #[test]
    fn test_quiet_flag_drives_quiet_output() {
        let cli = Cli::try_parse_from(["tryke", "test", "-q"]).unwrap();
        let verbosity = match cli.verbose.log_level() {
            Some(log::Level::Info) | Some(log::Level::Debug) | Some(log::Level::Trace) => {
                Verbosity::Verbose
            }
            Some(log::Level::Error) | None => Verbosity::Quiet,
            _ => Verbosity::Normal,
        };
        assert!(matches!(verbosity, Verbosity::Quiet));
    }

    #[test]
    fn test_item_id_with_file() {
        let item = TestItem {
            name: "test_add".into(),
            module_path: "tests.math".into(),
            file_path: Some(PathBuf::from("tests/math.py")),
            line_number: Some(10),
            display_name: None,
            expected_assertions: vec![],
        };
        assert_eq!(item.id(), "tests/math.py::test_add");
    }

    #[test]
    fn test_item_id_without_file() {
        let item = TestItem {
            name: "test_add".into(),
            module_path: "tests.math".into(),
            file_path: None,
            line_number: None,
            display_name: None,
            expected_assertions: vec![],
        };
        assert_eq!(item.id(), "tests.math::test_add");
    }

    #[test]
    fn watch_subcommand_parsed() {
        let cli = Cli::try_parse_from(["tryke", "watch"]).unwrap();
        assert!(matches!(cli.command, Commands::Watch { .. }));
    }

    #[test]
    fn watch_subcommand_with_reporter_parsed() {
        let cli = Cli::try_parse_from(["tryke", "watch", "--reporter", "json"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Watch {
                reporter: ReporterFormat::Json,
                ..
            }
        ));
    }

    #[test]
    fn server_subcommand_parsed() {
        let cli = Cli::try_parse_from(["tryke", "server", "--port", "9000"]).unwrap();
        assert!(matches!(cli.command, Commands::Server { port: 9000, .. }));
    }

    #[test]
    fn server_default_port() {
        let cli = Cli::try_parse_from(["tryke", "server"]).unwrap();
        assert!(matches!(cli.command, Commands::Server { port: 2337, .. }));
    }

    #[test]
    fn test_port_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "--port", "2337"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Test {
                port: Some(2337),
                ..
            }
        ));
    }

    #[test]
    fn test_port_flag_defaults_to_2337() {
        let cli = Cli::try_parse_from(["tryke", "test", "--port"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Test {
                port: Some(2337),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn run_cycle_runs_without_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(dir.path().join("test_x.py"), "@test\ndef test_x(): pass\n")
            .expect("write test file");
        let mut discoverer = Discoverer::new(dir.path());
        let mut reporter = TextReporter::new();
        let pool = WorkerPool::new(1, "python3", dir.path());
        assert!(
            run_cycle(&mut reporter, &mut discoverer, &pool)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn run_cycle_with_json_reporter() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let mut discoverer = Discoverer::new(dir.path());
        let mut reporter = JSONReporter::with_writer(Vec::new());
        let pool = WorkerPool::new(1, "python3", dir.path());
        assert!(
            run_cycle(&mut reporter, &mut discoverer, &pool)
                .await
                .is_ok()
        );
    }

    #[test]
    fn test_changed_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "--changed"]).unwrap();
        assert!(matches!(cli.command, Commands::Test { changed: true, .. }));
    }

    #[test]
    fn graph_subcommand_parsed() {
        let cli = Cli::try_parse_from(["tryke", "graph"]).unwrap();
        assert!(matches!(cli.command, Commands::Graph { .. }));
    }

    #[test]
    fn graph_subcommand_connected_only_parsed() {
        let cli = Cli::try_parse_from(["tryke", "graph", "--connected-only"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Graph {
                connected_only: true,
                ..
            }
        ));
    }

    #[test]
    fn run_graph_prints_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(dir.path().join("utils.py"), "def helper(): pass\n").expect("write");
        std::fs::write(
            dir.path().join("test_foo.py"),
            "from utils import helper\n@test\ndef test_foo(): pass\n",
        )
        .expect("write");
        assert!(run_graph(Some(dir.path()), false).is_ok());
    }

    #[test]
    fn run_graph_connected_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(dir.path().join("utils.py"), "def helper(): pass\n").expect("write");
        std::fs::write(
            dir.path().join("test_foo.py"),
            "from utils import helper\n@test\ndef test_foo(): pass\n",
        )
        .expect("write");
        std::fs::write(
            dir.path().join("test_isolated.py"),
            "@test\ndef test_isolated(): pass\n",
        )
        .expect("write");
        assert!(run_graph(Some(dir.path()), true).is_ok());
    }

    #[tokio::test]
    async fn run_changed_test_without_git_runs_all() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let mut reporter = TextReporter::new();
        // non-git directory → git_changed_files returns None → runs all tests (0 tests here)
        assert!(
            run_changed_test(&mut reporter, Some(dir.path()))
                .await
                .is_ok()
        );
    }
}
