use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Result;
use clap::Parser;
use log::{debug, warn};
use tokio_stream::StreamExt;
use tryke::cli::{Cli, Commands, ReporterFormat};
use tryke_config::load_effective_config;
use tryke_discovery::Discoverer;
use tryke_reporter::{
    DotReporter, JSONReporter, JUnitReporter, LlmReporter, ProgressReporter, Reporter,
    TextReporter, Verbosity,
};
use tryke_runner::{WorkerPool, resolve_python};
use tryke_types::filter::TestFilter;
use tryke_types::{ChangedSelectionSummary, RunSummary, TestOutcome};

fn worker_pool_size() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
}

struct DiscoverySelection {
    tests: Vec<tryke_types::TestItem>,
    changed_files: Option<usize>,
}

fn resolved_excludes(root: &Path, cli_excludes: &[String], cli_includes: &[String]) -> Vec<String> {
    if !cli_excludes.is_empty() {
        return cli_excludes.to_vec();
    }
    let includes = cli_includes
        .iter()
        .collect::<std::collections::HashSet<_>>();
    load_effective_config(root)
        .discovery
        .exclude
        .into_iter()
        .filter(|exclude| !includes.contains(exclude))
        .collect()
}

/// Discover tests, optionally restricting to changed files.
fn discover_tests(root: &Path, changed: bool, excludes: &[String]) -> DiscoverySelection {
    if changed {
        let mut discoverer = Discoverer::new_with_excludes(root, excludes);
        discoverer.rediscover();
        match git_changed_files(root) {
            Some(changed_files) if !changed_files.is_empty() => {
                debug!("--changed: {} git-changed files", changed_files.len());
                DiscoverySelection {
                    tests: discoverer.tests_for_changed(&changed_files),
                    changed_files: Some(changed_files.len()),
                }
            }
            Some(_) => {
                warn!("--changed: no changed files found via git, running all tests");
                DiscoverySelection {
                    tests: discoverer.tests(),
                    changed_files: None,
                }
            }
            None => {
                warn!("--changed: git unavailable or failed, running all tests");
                DiscoverySelection {
                    tests: discoverer.tests(),
                    changed_files: None,
                }
            }
        }
    } else {
        DiscoverySelection {
            tests: tryke_discovery::discover_from_with_excludes(root, excludes),
            changed_files: None,
        }
    }
}

async fn run_tests(
    reporter: &mut dyn Reporter,
    root: &Path,
    tests: Vec<tryke_types::TestItem>,
    maxfail: Option<usize>,
    workers: Option<usize>,
    discovery_duration: Option<Duration>,
    changed_selection: Option<ChangedSelectionSummary>,
) -> Result<()> {
    let python = resolve_python(root);
    let pool_size = workers.unwrap_or_else(|| tests.len().min(worker_pool_size()));
    let pool = WorkerPool::new(pool_size, &python, root);
    pool.warm().await;
    report_cycle(
        reporter,
        tests,
        &pool,
        maxfail,
        discovery_duration,
        changed_selection,
    )
    .await?;
    pool.shutdown();
    Ok(())
}

async fn report_cycle(
    reporter: &mut dyn Reporter,
    tests: Vec<tryke_types::TestItem>,
    pool: &WorkerPool,
    maxfail: Option<usize>,
    discovery_duration: Option<Duration>,
    changed_selection: Option<ChangedSelectionSummary>,
) -> Result<()> {
    use std::collections::HashSet;

    let file_count = tests
        .iter()
        .filter_map(|t| t.file_path.as_ref())
        .collect::<HashSet<_>>()
        .len();

    let start_time = chrono::Local::now().format("%H:%M:%S").to_string();

    let start = Instant::now();
    reporter.on_run_start(&tests);

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut xfailed = 0usize;
    let mut todo = 0usize;

    // Short-circuit skip/todo tests — no worker needed
    let (run_tests, shortcircuit): (Vec<_>, Vec<_>) = tests
        .into_iter()
        .partition(|t| t.skip.is_none() && t.todo.is_none());

    for t in shortcircuit {
        let outcome = if t.todo.is_some() {
            todo += 1;
            TestOutcome::Todo {
                description: t.todo.clone(),
            }
        } else {
            skipped += 1;
            TestOutcome::Skipped {
                reason: t.skip.clone(),
            }
        };
        let result = tryke_types::TestResult {
            test: t,
            outcome,
            duration: std::time::Duration::ZERO,
            stdout: String::new(),
            stderr: String::new(),
        };
        reporter.on_test_complete(&result);
    }

    let mut stream = pool.run(run_tests);
    while let Some(result) = stream.next().await {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } | TestOutcome::XPassed => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
            TestOutcome::Error { .. } => errors += 1,
            TestOutcome::XFailed { .. } => xfailed += 1,
            TestOutcome::Todo { .. } => todo += 1,
        }
        reporter.on_test_complete(&result);
        if let Some(max) = maxfail
            && failed >= max
        {
            break;
        }
    }

    reporter.on_run_complete(&RunSummary {
        passed,
        failed,
        skipped,
        errors,
        xfailed,
        todo,
        duration: discovery_duration.unwrap_or_default() + start.elapsed(),
        discovery_duration,
        test_duration: Some(start.elapsed()),
        file_count,
        start_time: Some(start_time),
        changed_selection,
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
    excludes: &[String],
    test_filter: &TestFilter,
    maxfail: Option<usize>,
    workers: Option<usize>,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new_with_excludes(root, excludes);

    let python = resolve_python(root);
    let pool_size = workers.unwrap_or_else(worker_pool_size);
    let pool = WorkerPool::new(pool_size, &python, root);
    pool.warm().await;

    clear_if_tty();
    let disc_start = Instant::now();
    let tests = test_filter.apply(discoverer.rediscover());
    let disc_dur = Some(disc_start.elapsed());
    report_cycle(reporter, tests, &pool, maxfail, disc_dur, None).await?;

    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let _debouncer = tryke_server::watcher::spawn_watcher(root, excludes, tx)?;

    for paths in &rx {
        let modules = discoverer.affected_modules(&paths);
        if !modules.is_empty() {
            pool.reload(modules).await;
        }
        discoverer.rediscover_changed(&paths);
        clear_if_tty();
        let disc_start = Instant::now();
        let tests = test_filter.apply(discoverer.tests_for_changed(&paths));
        let disc_dur = Some(disc_start.elapsed());
        report_cycle(reporter, tests, &pool, maxfail, disc_dur, None).await?;
    }

    pool.shutdown();
    Ok(())
}

fn git_paths(root: &Path, args: &[&str]) -> Option<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .args(args)
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

/// Collect changed files from git relative to `root`.
/// Includes tracked changes since HEAD and untracked files.
/// Returns `None` if git is unavailable or a command fails.
fn git_changed_files(root: &Path) -> Option<Vec<PathBuf>> {
    let tracked = git_paths(root, &["diff", "--name-only", "HEAD"])?;
    let untracked = git_paths(root, &["ls-files", "--others", "--exclude-standard"])?;
    let mut paths: Vec<PathBuf> = tracked
        .into_iter()
        .chain(untracked)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    paths.sort();
    Some(paths)
}

fn run_graph(
    root: Option<&Path>,
    excludes: &[String],
    connected_only: bool,
    changed: bool,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root_path = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new_with_excludes(root_path, excludes);
    discoverer.rediscover();

    let changed_files = if changed {
        match git_changed_files(root_path) {
            Some(paths) if !paths.is_empty() => Some(paths),
            Some(_) => {
                println!("No git-visible changed files found.");
                return Ok(());
            }
            None => {
                println!("Git unavailable or failed; cannot compute changed graph.");
                return Ok(());
            }
        }
    } else {
        None
    };

    let affected = changed_files
        .as_ref()
        .map(|paths| discoverer.affected_files(paths))
        .unwrap_or_default();
    let changed_set = changed_files
        .as_ref()
        .map(|paths| {
            paths
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let summary = discoverer.import_graph_summary();
    for entry in &summary {
        let full_path = root_path.join(&entry.file);
        if changed && !affected.contains(&full_path) {
            continue;
        }
        if connected_only && entry.imports.is_empty() && entry.imported_by.is_empty() {
            continue;
        }
        let label = if changed {
            if changed_set.contains(&full_path) {
                " [changed]"
            } else {
                " [affected]"
            }
        } else {
            ""
        };
        println!("{}{}", entry.file.display(), label);
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
    let use_progress = tryke_reporter::progress::supports_progress()
        && matches!(format, ReporterFormat::Text | ReporterFormat::Dot);

    match format {
        ReporterFormat::Text if use_progress => Box::new(ProgressReporter::new(
            TextReporter::with_verbosity(verbosity),
        )),
        ReporterFormat::Text => Box::new(TextReporter::with_verbosity(verbosity)),
        ReporterFormat::Dot if use_progress => Box::new(ProgressReporter::new(DotReporter::new())),
        ReporterFormat::Dot => Box::new(DotReporter::new()),
        ReporterFormat::Json => Box::new(JSONReporter::new()),
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
            exclude,
            collect_only,
            filter,
            markers,
            reporter,
            root,
            port,
            changed,
            fail_fast,
            maxfail,
            workers,
            include,
        } => {
            let resolved_maxfail = if *fail_fast { Some(1) } else { *maxfail };
            let mut rep = build_reporter(reporter, verbosity);
            if let Some(p) = port {
                if !exclude.is_empty() {
                    return Err(anyhow::anyhow!(
                        "--exclude is not supported with --port; start the server with --exclude instead"
                    ));
                }
                let root_path = root.clone().unwrap_or(env::current_dir()?);
                return tryke_server::Client::new(
                    *p,
                    filter.clone(),
                    paths.clone(),
                    markers.clone(),
                )
                .run(&root_path, &mut *rep);
            }

            let cwd = env::current_dir()?;
            let root_path = root.as_deref().unwrap_or(&cwd);
            let excludes = resolved_excludes(root_path, exclude, include);
            let test_filter = TestFilter::from_args(paths, filter.as_deref(), markers.as_deref())
                .map_err(|e| anyhow::anyhow!(e))?;

            let discovery_start = Instant::now();
            let discovered = discover_tests(root_path, *changed, &excludes);
            let tests = test_filter.apply(discovered.tests);
            let discovery_duration = discovery_start.elapsed();
            let changed_selection =
                discovered
                    .changed_files
                    .map(|changed_files| ChangedSelectionSummary {
                        changed_files,
                        affected_tests: tests.len(),
                    });

            if *collect_only {
                rep.on_collect_complete(&tests);
                Ok(())
            } else {
                rt.block_on(run_tests(
                    &mut *rep,
                    root_path,
                    tests,
                    resolved_maxfail,
                    *workers,
                    Some(discovery_duration),
                    changed_selection,
                ))
            }
        }
        Commands::Watch {
            exclude,
            filter,
            markers,
            reporter,
            root,
            fail_fast,
            maxfail,
            workers,
            include,
        } => {
            let resolved_maxfail = if *fail_fast { Some(1) } else { *maxfail };
            let mut rep = build_reporter(reporter, verbosity);
            let cwd = env::current_dir()?;
            let root_path = root.as_deref().unwrap_or(&cwd);
            let excludes = resolved_excludes(root_path, exclude, include);
            let test_filter = TestFilter::from_args(&[], filter.as_deref(), markers.as_deref())
                .map_err(|e| anyhow::anyhow!(e))?;
            rt.block_on(run_watch(
                &mut *rep,
                Some(root_path),
                &excludes,
                &test_filter,
                resolved_maxfail,
                *workers,
            ))
        }
        Commands::Server {
            port,
            root,
            exclude,
            include,
        } => {
            let root_path = root.clone().unwrap_or(env::current_dir()?);
            let excludes = resolved_excludes(&root_path, exclude, include);
            let server = tryke_server::Server::new(*port, root_path, excludes);
            rt.block_on(server.run())
        }
        Commands::Graph {
            root,
            exclude,
            include,
            connected_only,
            changed,
        } => {
            let cwd = env::current_dir()?;
            let root_path = root.as_deref().unwrap_or(&cwd);
            let excludes = resolved_excludes(root_path, exclude, include);
            run_graph(Some(root_path), &excludes, *connected_only, *changed)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap_verbosity_flag::log::LevelFilter;
    use tryke_reporter::{JSONReporter, TextReporter};
    use tryke_types::TestItem;

    use super::*;

    fn test_python_bin() -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        tryke_runner::resolve_python(&root)
    }

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
        report_cycle(reporter, discoverer.rediscover(), pool, None, None, None).await
    }

    #[tokio::test]
    async fn test_command_text() {
        let mut reporter = TextReporter::with_writer(Vec::new());
        let root = cwd();
        let excludes = resolved_excludes(&root, &[], &[]);
        let tests = discover_tests(&root, false, &excludes).tests;
        assert!(
            run_tests(&mut reporter, &root, tests, None, None, None, None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_command_json() {
        let mut reporter = JSONReporter::with_writer(Vec::new());
        let root = cwd();
        let excludes = resolved_excludes(&root, &[], &[]);
        let tests = discover_tests(&root, false, &excludes).tests;
        assert!(
            run_tests(&mut reporter, &root, tests, None, None, None, None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_command_dot() {
        let mut reporter = DotReporter::with_writer(Vec::new());
        let root = cwd();
        let excludes = resolved_excludes(&root, &[], &[]);
        let tests = discover_tests(&root, false, &excludes).tests;
        assert!(
            run_tests(&mut reporter, &root, tests, None, None, None, None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_command_junit() {
        let mut reporter = JUnitReporter::with_writer(Vec::new());
        let root = cwd();
        let excludes = resolved_excludes(&root, &[], &[]);
        let tests = discover_tests(&root, false, &excludes).tests;
        assert!(
            run_tests(&mut reporter, &root, tests, None, None, None, None)
                .await
                .is_ok()
        );
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
            let display = test.display_name.as_deref().unwrap_or(&test.name);
            assert!(out.contains(display), "missing {display} in output");
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
            ..Default::default()
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
            ..Default::default()
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
                ..Default::default()
            },
            TestItem {
                name: "test_b".into(),
                module_path: "m".into(),
                file_path: Some(PathBuf::from("b.py")),
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
                ..Default::default()
            },
            TestItem {
                name: "test_a2".into(),
                module_path: "m".into(),
                file_path: Some(PathBuf::from("a.py")),
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
                ..Default::default()
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
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_x.py"),
            "from tryke import test\n\n@test\ndef test_x(): pass\n",
        )
        .expect("write test file");
        let mut discoverer = Discoverer::new(dir.path());
        let mut reporter = TextReporter::new();
        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_dir],
        );
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
        let pool = WorkerPool::new(1, &test_python_bin(), dir.path());
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
    fn test_exclude_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "-e", "benchmarks/suites"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Test { exclude, .. } if exclude == &["benchmarks/suites"]
        ));
    }

    #[test]
    fn test_include_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "--include", "benchmarks/suites"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Test {
                include,
                ..
            } if include == &["benchmarks/suites"]
        ));
    }

    #[test]
    fn resolved_excludes_reads_pyproject_when_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"benchmarks/suites\"]\n",
        )
        .expect("write pyproject.toml");

        let excludes = resolved_excludes(dir.path(), &[], &[]);
        assert_eq!(excludes, vec!["benchmarks/suites"]);
    }

    #[test]
    fn resolved_excludes_removes_included_config_excludes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"benchmarks/suites\", \"generated\"]\n",
        )
        .expect("write pyproject.toml");

        let excludes = resolved_excludes(dir.path(), &[], &["benchmarks/suites".into()]);
        assert_eq!(excludes, vec!["generated"]);
    }

    #[test]
    fn resolved_excludes_prefers_cli_excludes_over_includes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"benchmarks/suites\"]\n",
        )
        .expect("write pyproject.toml");

        let excludes = resolved_excludes(
            dir.path(),
            &["tmp".into(), "cache".into()],
            &["benchmarks/suites".into()],
        );
        assert_eq!(excludes, vec!["tmp", "cache"]);
    }

    #[test]
    fn graph_subcommand_parsed() {
        let cli = Cli::try_parse_from(["tryke", "graph"]).unwrap();
        assert!(matches!(cli.command, Commands::Graph { .. }));
    }

    #[test]
    fn graph_subcommand_changed_parsed() {
        let cli = Cli::try_parse_from(["tryke", "graph", "--changed"]).unwrap();
        assert!(matches!(cli.command, Commands::Graph { changed: true, .. }));
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
        assert!(run_graph(Some(dir.path()), &[], false, false).is_ok());
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
        assert!(run_graph(Some(dir.path()), &[], true, false).is_ok());
    }

    fn init_git_repo(dir: &tempfile::TempDir) {
        fn run(dir: &tempfile::TempDir, args: &[&str]) {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("run git");
            assert!(status.success(), "git {:?} failed", args);
        }

        run(dir, &["init"]);
        run(dir, &["config", "user.email", "tryke@example.com"]);
        run(dir, &["config", "user.name", "Tryke Tests"]);
    }

    #[test]
    fn git_changed_files_includes_untracked() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(dir.path().join("tracked.py"), "def helper(): pass\n").expect("write");
        init_git_repo(&dir);

        let add_status = std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .expect("git add");
        assert!(add_status.success());
        let commit_status = std::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir.path())
            .status()
            .expect("git commit");
        assert!(commit_status.success());

        std::fs::write(
            dir.path().join("test_new.py"),
            "@test\ndef test_new(): pass\n",
        )
        .expect("write untracked file");

        let changed = git_changed_files(dir.path()).expect("git changed files");
        assert!(changed.contains(&dir.path().join("test_new.py")));
    }

    #[tokio::test]
    async fn run_changed_test_without_git_runs_all() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let mut reporter = TextReporter::new();
        // non-git directory → git_changed_files returns None → discover_tests runs all (0 here)
        let tests = discover_tests(dir.path(), true, &[]).tests;
        assert!(
            run_tests(&mut reporter, dir.path(), tests, None, None, None, None)
                .await
                .is_ok()
        );
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

    #[test]
    fn test_workers_short_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "-j", "4"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Test {
                workers: Some(4),
                ..
            }
        ));
    }

    #[test]
    fn test_workers_long_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "--workers", "8"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Test {
                workers: Some(8),
                ..
            }
        ));
    }

    #[test]
    fn test_workers_default_is_none() {
        let cli = Cli::try_parse_from(["tryke", "test"]).unwrap();
        assert!(matches!(cli.command, Commands::Test { workers: None, .. }));
    }

    #[test]
    fn watch_workers_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "watch", "-j", "2"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Watch {
                workers: Some(2),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn integration_python_worker_runs_tests() {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_example.py"),
            "\
from tryke import test, expect

@test
def test_passing():
    expect(1 + 1).to_equal(2)

@test
def test_failing():
    expect(1 + 1).to_equal(3)
",
        )
        .expect("write test file");

        let tests = discover_tests(dir.path(), false, &[]).tests;
        assert_eq!(tests.len(), 2);

        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_dir],
        );
        pool.warm().await;
        let mut results: Vec<_> = pool.run(tests).collect().await;
        results.sort_by(|a, b| a.test.name.cmp(&b.test.name));

        assert_eq!(results.len(), 2);
        assert!(
            matches!(results[0].outcome, TestOutcome::Failed { .. }),
            "test_failing should fail, got {:?}",
            results[0].outcome
        );
        assert!(
            matches!(results[1].outcome, TestOutcome::Passed),
            "test_passing should pass, got {:?}",
            results[1].outcome
        );
        for r in &results {
            assert!(
                !matches!(r.outcome, TestOutcome::Error { .. }),
                "unexpected worker error: {:?}",
                r.outcome
            );
        }

        pool.shutdown();
    }
}
