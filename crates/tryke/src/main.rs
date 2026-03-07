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
use tryke_reporter::{
    DotReporter, JSONReporter, JUnitReporter, LlmReporter, Reporter, TextReporter, Verbosity,
};
use tryke_runner::{WorkerPool, resolve_python};
use tryke_types::filter::TestFilter;
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
    Llm,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Test {
        /// File paths or file:line specs to restrict collection
        paths: Vec<String>,
        #[arg(long)]
        collect_only: bool,
        /// Filter expression (e.g. "math and not slow")
        #[arg(short = 'k', long = "filter")]
        filter: Option<String>,
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
        /// Filter expression (e.g. "math and not slow")
        #[arg(short = 'k', long = "filter")]
        filter: Option<String>,
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

/// Discover tests, optionally restricting to changed files.
fn discover_tests(root: &Path, changed: bool) -> Vec<tryke_types::TestItem> {
    if changed {
        let mut discoverer = Discoverer::new(root);
        discoverer.rediscover();
        match git_changed_files(root) {
            Some(changed_files) if !changed_files.is_empty() => {
                debug!("--changed: {} git-changed files", changed_files.len());
                discoverer.tests_for_changed(&changed_files)
            }
            Some(_) => {
                warn!("--changed: no changed files found via git, running all tests");
                discoverer.tests()
            }
            None => {
                warn!("--changed: git unavailable or failed, running all tests");
                discoverer.tests()
            }
        }
    } else {
        tryke_discovery::discover_from(root)
    }
}

async fn run_tests(
    reporter: &mut dyn Reporter,
    root: &Path,
    tests: Vec<tryke_types::TestItem>,
) -> Result<()> {
    let python = resolve_python(root);
    let pool = WorkerPool::new(worker_pool_size(), &python, root);
    pool.warm().await;
    report_cycle(reporter, tests, &pool).await?;
    pool.shutdown();
    Ok(())
}

async fn report_cycle(
    reporter: &mut dyn Reporter,
    tests: Vec<tryke_types::TestItem>,
    pool: &WorkerPool,
) -> Result<()> {
    let start = Instant::now();
    reporter.on_run_start(&tests);

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;

    let mut stream = pool.run(tests);
    while let Some(result) = stream.next().await {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
            TestOutcome::Error { .. } => errors += 1,
        }
        reporter.on_test_complete(&result);
    }

    reporter.on_run_complete(&RunSummary {
        passed,
        failed,
        skipped,
        errors,
        duration: start.elapsed(),
    });

    Ok(())
}

fn clear_if_tty() {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        let _ = clearscreen::clear();
    }
}

async fn run_watch(
    reporter: &mut dyn Reporter,
    root: Option<&Path>,
    test_filter: &TestFilter,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new(root);

    let python = resolve_python(root);
    let pool = WorkerPool::new(worker_pool_size(), &python, root);
    pool.warm().await;

    clear_if_tty();
    let tests = test_filter.apply(discoverer.rediscover());
    report_cycle(reporter, tests, &pool).await?;

    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let _debouncer = tryke_server::watcher::spawn_watcher(root, tx)?;

    for paths in &rx {
        let modules = discoverer.affected_modules(&paths);
        if !modules.is_empty() {
            pool.reload(modules).await;
        }
        discoverer.rediscover_changed(&paths);
        clear_if_tty();
        let tests = test_filter.apply(discoverer.tests_for_changed(&paths));
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

fn run_graph(root: Option<&Path>, connected_only: bool) -> Result<()> {
    let cwd = env::current_dir()?;
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
        ReporterFormat::Llm => Box::new(LlmReporter::new()),
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
            paths,
            collect_only,
            filter,
            reporter,
            root,
            port,
            changed,
        } => {
            let mut rep = build_reporter(reporter, verbosity);
            if let Some(p) = port {
                let root_path = root.clone().unwrap_or(env::current_dir()?);
                return tryke_server::Client::new(*p).run(&root_path, &mut *rep);
            }

            let cwd = env::current_dir()?;
            let root_path = root.as_deref().unwrap_or(&cwd);
            let test_filter =
                TestFilter::from_args(paths, filter.as_deref()).map_err(|e| anyhow::anyhow!(e))?;

            let tests = discover_tests(root_path, *changed);
            let tests = test_filter.apply(tests);

            if *collect_only {
                rep.on_collect_complete(&tests);
                Ok(())
            } else {
                rt.block_on(run_tests(&mut *rep, root_path, tests))
            }
        }
        Commands::Watch {
            filter,
            reporter,
            root,
        } => {
            let mut rep = build_reporter(reporter, verbosity);
            let test_filter =
                TestFilter::from_args(&[], filter.as_deref()).map_err(|e| anyhow::anyhow!(e))?;
            rt.block_on(run_watch(&mut *rep, root.as_deref(), &test_filter))
        }
        Commands::Server { port, root } => {
            let root_path = root.clone().unwrap_or(env::current_dir()?);
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

    fn group_tests_by_file(tests: Vec<TestItem>) -> Vec<(Option<PathBuf>, Vec<TestItem>)> {
        let mut index: std::collections::HashMap<Option<PathBuf>, usize> =
            std::collections::HashMap::new();
        let mut groups: Vec<(Option<PathBuf>, Vec<TestItem>)> = Vec::new();
        for test in tests {
            let key = test.file_path.clone();
            if let Some(&idx) = index.get(&key) {
                groups[idx].1.push(test);
            } else {
                index.insert(key.clone(), groups.len());
                groups.push((key, vec![test]));
            }
        }
        groups
    }

    fn cwd() -> PathBuf {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    async fn run_cycle(
        reporter: &mut dyn Reporter,
        discoverer: &mut Discoverer,
        pool: &WorkerPool,
    ) -> Result<()> {
        report_cycle(reporter, discoverer.rediscover(), pool).await
    }

    #[tokio::test]
    async fn test_command_text() {
        let mut reporter = TextReporter::new();
        let root = cwd();
        let tests = discover_tests(&root, false);
        assert!(run_tests(&mut reporter, &root, tests).await.is_ok());
    }

    #[tokio::test]
    async fn test_command_json() {
        let mut reporter = JSONReporter::new();
        let root = cwd();
        let tests = discover_tests(&root, false);
        assert!(run_tests(&mut reporter, &root, tests).await.is_ok());
    }

    #[tokio::test]
    async fn test_command_dot() {
        let mut reporter = DotReporter::new();
        let root = cwd();
        let tests = discover_tests(&root, false);
        assert!(run_tests(&mut reporter, &root, tests).await.is_ok());
    }

    #[tokio::test]
    async fn test_command_junit() {
        let mut reporter = JUnitReporter::new();
        let root = cwd();
        let tests = discover_tests(&root, false);
        assert!(run_tests(&mut reporter, &root, tests).await.is_ok());
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
        let tests = tryke_discovery::discover().expect("current_dir");
        reporter.on_collect_complete(&tests);
        let out = String::from_utf8_lossy(&reporter.into_writer()).into_owned();
        for test in &tests {
            assert!(out.contains(&test.id()), "missing {} in output", test.id());
        }
        assert!(out.contains("tests collected."));
    }

    #[test]
    fn test_collect_only_json() {
        let mut reporter = JSONReporter::with_writer(Vec::new());
        let tests = tryke_discovery::discover().expect("current_dir");
        reporter.on_collect_complete(&tests);
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
    fn group_tests_by_file_handles_unsorted_input() {
        let tests = vec![
            TestItem {
                name: "test_a".into(),
                module_path: "m".into(),
                file_path: Some(PathBuf::from("a.py")),
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
            TestItem {
                name: "test_b".into(),
                module_path: "m".into(),
                file_path: Some(PathBuf::from("b.py")),
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
            TestItem {
                name: "test_a2".into(),
                module_path: "m".into(),
                file_path: Some(PathBuf::from("a.py")),
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
        ];
        let groups = group_tests_by_file(tests);
        assert_eq!(
            groups.len(),
            2,
            "a.py tests should be merged into one group"
        );
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].1.len(), 1);
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
        // non-git directory → git_changed_files returns None → discover_tests runs all (0 here)
        let tests = discover_tests(dir.path(), true);
        assert!(run_tests(&mut reporter, dir.path(), tests).await.is_ok());
    }

    #[test]
    fn test_filter_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "-k", "test_add"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Test {
                filter: Some(f),
                ..
            } if f == "test_add"
        ));
    }

    #[test]
    fn test_filter_long_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "--filter", "math and add"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Test {
                filter: Some(f),
                ..
            } if f == "math and add"
        ));
    }

    #[test]
    fn test_positional_paths_parsed() {
        let cli =
            Cli::try_parse_from(["tryke", "test", "tests/math.py", "tests/utils.py"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Test { paths, .. } if paths == &["tests/math.py", "tests/utils.py"]
        ));
    }

    #[test]
    fn test_paths_and_filter_combined() {
        let cli =
            Cli::try_parse_from(["tryke", "test", "tests/math.py", "-k", "test_add"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Test {
                paths,
                filter: Some(f),
                ..
            } if paths == &["tests/math.py"] && f == "test_add"
        ));
    }

    #[test]
    fn watch_filter_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "watch", "-k", "test_add"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Watch {
                filter: Some(f),
                ..
            } if f == "test_add"
        ));
    }
}
