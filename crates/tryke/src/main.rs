use std::{
    env,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use clap_verbosity_flag::{Verbosity as LogVerbosity, WarnLevel};
use log::debug;
use tryke_discovery::Discoverer;
use tryke_reporter::{DotReporter, JSONReporter, JUnitReporter, Reporter, TextReporter, Verbosity};
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

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
        #[arg(long)]
        port: Option<u16>,
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
}

fn fake_results(tests: &[TestItem]) -> Vec<TestResult> {
    tests
        .iter()
        .map(|test| {
            let outcome = TestOutcome::Passed;
            let duration = Duration::from_millis(0);
            TestResult {
                test: test.clone(),
                outcome,
                duration,
                stdout: String::new(),
                stderr: String::new(),
            }
        })
        .collect()
}

fn run_test(reporter: &mut dyn Reporter, root: Option<&Path>) -> Result<()> {
    let start = Instant::now();

    let tests = match root {
        Some(r) => tryke_discovery::discover_from(r),
        None => tryke_discovery::discover(),
    };
    reporter.on_run_start(&tests);

    let results = fake_results(&tests);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    for result in &results {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
        }
        reporter.on_test_complete(result);
    }

    reporter.on_run_complete(&RunSummary {
        passed,
        failed,
        skipped,
        duration: start.elapsed(),
    });

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

fn report_cycle(reporter: &mut dyn Reporter, tests: Vec<TestItem>) -> Result<()> {
    let start = Instant::now();
    reporter.on_run_start(&tests);

    let results = fake_results(&tests);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    for result in &results {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
        }
        reporter.on_test_complete(result);
    }

    reporter.on_run_complete(&RunSummary {
        passed,
        failed,
        skipped,
        duration: start.elapsed(),
    });

    Ok(())
}

fn run_cycle(reporter: &mut dyn Reporter, discoverer: &mut Discoverer) -> Result<()> {
    report_cycle(reporter, discoverer.rediscover())
}

fn clear_if_tty() {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        let _ = clearscreen::clear();
    }
}

fn run_watch(reporter: &mut dyn Reporter, root: Option<&Path>) -> Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new(root);

    clear_if_tty();
    run_cycle(reporter, &mut discoverer)?;

    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let _debouncer = tryke_server::watcher::spawn_watcher(root, tx)?;

    for paths in &rx {
        clear_if_tty();
        report_cycle(reporter, discoverer.rediscover_changed(&paths))?;
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

    match &cli.command {
        Commands::Test {
            collect_only,
            reporter,
            root,
            port,
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
            } else {
                run_test(&mut *rep, root_ref)
            }
        }
        Commands::Watch { reporter, root } => {
            let mut rep = build_reporter(reporter, verbosity);
            run_watch(&mut *rep, root.as_deref())
        }
        Commands::Server { port, root } => {
            let root_path = root
                .clone()
                .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let server = tryke_server::Server::new(*port, root_path);
            tokio::runtime::Runtime::new()?.block_on(server.run())
        }
    }
}

#[cfg(test)]
mod tests {
    use clap_verbosity_flag::log::LevelFilter;

    use super::*;

    #[test]
    fn test_command_text() {
        let mut reporter = TextReporter::new();
        assert!(run_test(&mut reporter, None).is_ok());
    }

    #[test]
    fn test_command_json() {
        let mut reporter = JSONReporter::new();
        assert!(run_test(&mut reporter, None).is_ok());
    }

    #[test]
    fn test_command_dot() {
        let mut reporter = DotReporter::new();
        assert!(run_test(&mut reporter, None).is_ok());
    }

    #[test]
    fn test_command_junit() {
        let mut reporter = JUnitReporter::new();
        assert!(run_test(&mut reporter, None).is_ok());
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
    fn run_cycle_runs_without_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(dir.path().join("test_x.py"), "@test\ndef test_x(): pass\n")
            .expect("write test file");
        let mut discoverer = Discoverer::new(dir.path());
        let mut reporter = TextReporter::new();
        assert!(run_cycle(&mut reporter, &mut discoverer).is_ok());
    }

    #[test]
    fn run_cycle_with_json_reporter() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let mut discoverer = Discoverer::new(dir.path());
        let mut reporter = JSONReporter::with_writer(Vec::new());
        assert!(run_cycle(&mut reporter, &mut discoverer).is_ok());
    }
}
