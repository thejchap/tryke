use std::{env, time::Instant};

use anyhow::Result;
use clap::Parser;
use log::debug;
use tryke::cli::{Cli, Commands, ReporterFormat};
use tryke::discovery::{
    discover_tests, discover_tests_changed_first, discover_tests_for_paths, resolved_excludes,
};
use tryke::execution::run_tests;
use tryke::graph::{run_fixture_graph, run_graph};
use tryke::watch::run_watch;
use tryke_reporter::{
    DotReporter, JSONReporter, JUnitReporter, LlmReporter, ProgressReporter, Reporter,
    TextReporter, Verbosity,
};
use tryke_types::ChangedSelectionSummary;
use tryke_types::filter::TestFilter;

fn build_reporter(format: &ReporterFormat, verbosity: Verbosity) -> Box<dyn Reporter> {
    let use_progress = tryke_reporter::progress::supports_progress()
        && matches!(format, ReporterFormat::Text | ReporterFormat::Dot);

    if use_progress {
        // ProgressReporter emits OSC 9;4 "set progress" on every test
        // completion. On Ctrl+C, `on_run_complete` (which emits the
        // clear sequence) never runs, so the terminal's progress bar
        // would freeze. Install a signal handler that clears it first.
        tryke_reporter::progress::install_cleanup_handler();
    }

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
            changed_first,
            base_branch,
            fail_fast,
            maxfail,
            workers,
            dist,
            include,
        } => {
            if base_branch.is_some() && !changed && !changed_first {
                return Err(anyhow::anyhow!(
                    "--base-branch requires --changed or --changed-first"
                ));
            }
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
            // Fast path: explicit paths without change-based selection
            // skip the full project walk and import-graph build. The
            // post-filter (`test_filter.apply` below) still runs to
            // honor `:line` specs, `--filter`, and `--markers`.
            let discovered = if !paths.is_empty() && !*changed && !*changed_first {
                discover_tests_for_paths(root_path, &test_filter.path_specs, &excludes)
            } else if *changed_first {
                discover_tests_changed_first(root_path, base_branch.as_deref(), &excludes)
            } else {
                discover_tests(root_path, *changed, base_branch.as_deref(), &excludes)
            };
            for warning in &discovered.warnings {
                rep.on_discovery_warning(warning);
            }
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
                    &discovered.hooks,
                    resolved_maxfail,
                    *workers,
                    (*dist).into(),
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
            dist,
            include,
            all,
        } => {
            let resolved_maxfail = if *fail_fast { Some(1) } else { *maxfail };
            let mut rep = build_reporter(reporter, verbosity);
            rep.set_subcommand_label("tryke watch");
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
                (*dist).into(),
                *all,
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
            base_branch,
            fixtures,
        } => {
            if base_branch.is_some() && !changed {
                return Err(anyhow::anyhow!("--base-branch requires --changed"));
            }
            let cwd = env::current_dir()?;
            let root_path = root.as_deref().unwrap_or(&cwd);
            let excludes = resolved_excludes(root_path, exclude, include);
            if *fixtures {
                run_fixture_graph(Some(root_path), &excludes)
            } else {
                run_graph(
                    Some(root_path),
                    &excludes,
                    *connected_only,
                    *changed,
                    base_branch.as_deref(),
                )
            }
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

    #[test]
    fn watch_all_flag_defaults_to_false() {
        let cli = Cli::try_parse_from(["tryke", "watch"]).unwrap();
        assert!(matches!(cli.command, Commands::Watch { all: false, .. }));
    }

    #[test]
    fn watch_all_long_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "watch", "--all"]).unwrap();
        assert!(matches!(cli.command, Commands::Watch { all: true, .. }));
    }

    #[test]
    fn watch_all_short_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "watch", "-a"]).unwrap();
        assert!(matches!(cli.command, Commands::Watch { all: true, .. }));
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

    // --- CLI parsing tests for new flags ---

    #[test]
    fn test_changed_with_base_branch_parsed() {
        let cli =
            Cli::try_parse_from(["tryke", "test", "--changed", "--base-branch", "main"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Test {
                changed: true,
                base_branch: Some(b),
                ..
            } if b == "main"
        ));
    }

    #[test]
    fn test_changed_first_flag_parsed() {
        let cli = Cli::try_parse_from(["tryke", "test", "--changed-first"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Test {
                changed_first: true,
                ..
            }
        ));
    }

    #[test]
    fn test_changed_first_conflicts_with_changed() {
        let result = Cli::try_parse_from(["tryke", "test", "--changed", "--changed-first"]);
        assert!(
            result.is_err(),
            "--changed and --changed-first should conflict"
        );
    }

    #[test]
    fn test_changed_first_with_base_branch_parsed() {
        let cli =
            Cli::try_parse_from(["tryke", "test", "--changed-first", "--base-branch", "main"])
                .unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Test {
                changed_first: true,
                base_branch: Some(b),
                ..
            } if b == "main"
        ));
    }

    #[test]
    fn graph_changed_with_base_branch_parsed() {
        let cli =
            Cli::try_parse_from(["tryke", "graph", "--changed", "--base-branch", "main"]).unwrap();
        assert!(matches!(
            &cli.command,
            Commands::Graph {
                changed: true,
                base_branch: Some(b),
                ..
            } if b == "main"
        ));
    }
}
