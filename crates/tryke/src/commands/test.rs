use std::{
    env,
    future::Future,
    time::{Duration, Instant},
};

use anyhow::Result;
use console::{Key, Term};
use log::{LevelFilter, debug};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tryke_config::{Project, ProjectMetadata};
use tryke_discovery::{Discoverer, DiscoveryOptions};
use tryke_reporter::{Reporter, build_reporter, reporter::WatchIdleInfo};
use tryke_runner::{DistMode, WorkerPool, WorkerPoolOptions, partition_with_hooks};
use tryke_types::{
    ChangedSelectionSummary, DiscoveryWarning, DiscoveryWarningKind, HookItem, RunSummary,
    TestOutcome, filter::TestFilter,
};
use tryke_watcher::{FileChangeBatch, FileWatcher};

use super::CommandOrigin;
use crate::ExitStatus;
use crate::cli::{GlobalArgs, TestArgs};
use crate::logging::LogConfig;

#[derive(Debug, Eq, PartialEq)]
enum Interruptible<T> {
    Completed(T),
    Interrupted,
}

async fn run_interruptibly<T>(
    run: impl Future<Output = Result<T>>,
    cancellation: &CancellationToken,
) -> Result<Interruptible<T>> {
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Ok(Interruptible::Interrupted),
        result = run => result.map(Interruptible::Completed),
    }
}

fn resolve_interruptible<T>(
    result: Result<Interruptible<T>>,
    reporter: &mut dyn Reporter,
) -> Result<Option<T>> {
    match result {
        Ok(Interruptible::Completed(value)) => Ok(Some(value)),
        Ok(Interruptible::Interrupted) => {
            reporter.cleanup();
            Ok(None)
        }
        Err(error) => {
            reporter.cleanup();
            Err(error)
        }
    }
}

pub(crate) async fn run_test_command(
    args: TestArgs,
    global: &GlobalArgs,
    origin: CommandOrigin,
    logging: LogConfig,
    cancellation: CancellationToken,
) -> Result<ExitStatus> {
    if args.base_branch.is_some() && !args.changed && !args.changed_first {
        return Err(anyhow::anyhow!(
            "--base-branch requires --changed or --changed-first"
        ));
    }

    let log_level = logging.level();
    let verbosity = logging.reporter_verbosity();

    let maxfail = if args.fail_fast {
        Some(1)
    } else {
        args.maxfail
    };

    let mut reporter = build_reporter(args.reporter.kind(), verbosity, global.no_progress);
    let cwd = env::current_dir()?;

    let mut metadata = ProjectMetadata::new(args.root.as_deref().unwrap_or(&cwd));
    metadata.apply_configuration_file();
    metadata.apply_cli_args(args.project_options(global));
    let project = Project::from_metadata(metadata);

    if args.watch {
        reporter.set_subcommand_label(match origin {
            CommandOrigin::Bare => "tryke",
            CommandOrigin::Explicit => "tryke test --watch",
        });

        reporter.set_watch_hint(Some("Waiting for file changes...".into()));

        let test_filter =
            TestFilter::from_args(&[], args.filter.as_deref(), args.markers.as_deref())
                .map_err(|error| anyhow::anyhow!(error))?;

        let result = run_watch(
            &mut *reporter,
            &project,
            log_level,
            &test_filter,
            maxfail,
            args.workers,
            args.dist.into(),
            args.all,
            args.now,
            &cancellation,
        )
        .await;

        if resolve_interruptible(result, &mut *reporter)?.is_none() {
            return Ok(ExitStatus::Interrupted);
        }

        return Ok(ExitStatus::Success);
    }

    let test_filter =
        TestFilter::from_args(&args.paths, args.filter.as_deref(), args.markers.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;

    let discovery_start = Instant::now();

    let mut discoverer = Discoverer::new(&project);
    let discovered = discoverer.discover(DiscoveryOptions {
        paths: &test_filter.path_specs,
        changed: args.changed,
        changed_first: args.changed_first,
        base_branch: args.base_branch.as_deref(),
    });

    for warning in &discovered.warnings {
        reporter.on_discovery_warning(warning);
    }

    let tests = test_filter.apply(discovered.tests);

    let discovery_duration = discovery_start.elapsed();

    let changed_selection = discovered
        .changed_files
        .map(|changed_files| ChangedSelectionSummary {
            changed_files,
            affected_tests: tests.len(),
        });

    if args.collect_only {
        reporter.on_collect_complete(&tests);
        return Ok(ExitStatus::Success);
    }

    // let mut _snapshots = SnapshotRun::begin(SnapshotRunOptions {
    //     root: project.root().into(),
    //     mode: args.snapshot_mode.to_wire(),
    // })?;

    let result = run_tests(
        &mut *reporter,
        &project,
        log_level,
        tests,
        &discovered.hooks,
        maxfail,
        args.workers,
        args.dist.into(),
        Some(discovery_duration),
        changed_selection,
        &cancellation,
    )
    .await;

    let Some(summary) = resolve_interruptible(result, &mut *reporter)? else {
        return Ok(ExitStatus::Interrupted);
    };

    if summary.failed > 0 || summary.errors > 0 {
        Ok(ExitStatus::Failure)
    } else {
        Ok(ExitStatus::Success)
    }
}

#[expect(clippy::too_many_arguments)]
async fn run_tests(
    reporter: &mut dyn Reporter,
    project: &Project,
    log_level: LevelFilter,
    tests: Vec<tryke_types::TestItem>,
    hooks: &[HookItem],
    maxfail: Option<usize>,
    workers: usize,
    dist: DistMode,
    discovery_duration: Option<Duration>,
    changed_selection: Option<ChangedSelectionSummary>,
    cancellation: &CancellationToken,
) -> Result<Interruptible<RunSummary>> {
    let pool = WorkerPool::spawn(
        project,
        WorkerPoolOptions {
            size: workers,
            python_path: None,
            log_level,
            warm: true,
        },
    )
    .await;

    let run_result = run_interruptibly(
        report_cycle(
            reporter,
            tests,
            hooks,
            &pool,
            maxfail,
            dist,
            discovery_duration,
            changed_selection,
        ),
        cancellation,
    )
    .await;
    let shutdown_result = pool.shutdown().await;

    combine_shutdown(run_result, shutdown_result)
}

fn combine_shutdown<T>(result: Result<T>, shutdown_result: Result<()>) -> Result<T> {
    match (result, shutdown_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(shutdown_error)) => Err(shutdown_error),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(shutdown_error)) => Err(error.context(format!(
            "Worker pool shutdown also failed: {shutdown_error:#}"
        ))),
    }
}

fn flush_buffer(
    file: &Option<std::path::PathBuf>,
    buffers: &mut std::collections::HashMap<
        Option<std::path::PathBuf>,
        Vec<(usize, tryke_types::TestResult)>,
    >,
    reporter: &mut dyn Reporter,
) {
    if let Some(mut buf) = buffers.remove(file) {
        buf.sort_by_key(|(idx, _)| *idx);
        for (_, result) in buf {
            reporter.on_test_complete(&result);
        }
    }
}

#[expect(clippy::too_many_arguments)]
async fn report_cycle(
    reporter: &mut dyn Reporter,
    tests: Vec<tryke_types::TestItem>,
    hooks: &[HookItem],
    pool: &WorkerPool,
    maxfail: Option<usize>,
    dist: DistMode,
    discovery_duration: Option<Duration>,
    changed_selection: Option<ChangedSelectionSummary>,
) -> Result<RunSummary> {
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    let file_count = tests
        .iter()
        .filter_map(|t| t.file_path.as_ref())
        .collect::<HashSet<_>>()
        .len();

    let start_time = chrono::Local::now().format("%H:%M:%S").to_string();

    // Build the discovery-order index and per-file expected counts before
    // partitioning so short-circuit tests are included.
    let discovery_order: HashMap<String, usize> =
        tests.iter().enumerate().map(|(i, t)| (t.id(), i)).collect();

    let mut expected_per_file: HashMap<Option<PathBuf>, usize> = HashMap::new();
    for t in &tests {
        *expected_per_file.entry(t.file_path.clone()).or_default() += 1;
    }

    let start = Instant::now();
    reporter.on_run_start(&tests);

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut xfailed = 0usize;
    let mut todo = 0usize;

    type FileBuffer = Vec<(usize, tryke_types::TestResult)>;
    let mut buffers: HashMap<Option<PathBuf>, FileBuffer> = HashMap::new();

    // Short-circuit skip/todo tests and buffer them instead of reporting eagerly.
    let (runnable_tests, shortcircuit): (Vec<_>, Vec<_>) = tests
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
            duration: Duration::ZERO,
            stdout: String::new(),
            stderr: String::new(),
        };
        let idx = discovery_order
            .get(&result.test.id())
            .copied()
            .unwrap_or(usize::MAX);
        let file = result.test.file_path.clone();
        buffers.entry(file).or_default().push((idx, result));
    }

    let mut hit_maxfail = false;
    let partition = partition_with_hooks(runnable_tests, hooks, dist);
    for warning in &partition.warnings {
        reporter.on_discovery_warning(warning);
    }

    let mut stream = pool.submit(partition.units);

    while let Some(result) = stream.next().await {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } | TestOutcome::XPassed => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
            TestOutcome::Error { .. } => errors += 1,
            TestOutcome::XFailed { .. } => xfailed += 1,
            TestOutcome::Todo { .. } => todo += 1,
        }

        let idx = discovery_order
            .get(&result.test.id())
            .copied()
            .unwrap_or(usize::MAX);

        let file = result.test.file_path.clone();

        buffers.entry(file.clone()).or_default().push((idx, result));

        // Flush when this file's buffer is complete.
        if let Some(&expected) = expected_per_file.get(&file)
            && buffers.get(&file).is_some_and(|b| b.len() >= expected)
        {
            flush_buffer(&file, &mut buffers, reporter);
        }

        if let Some(max) = maxfail
            && failed >= max
        {
            hit_maxfail = true;
            break;
        }
    }

    // Flush any remaining buffered files, including partial files from maxfail.
    if hit_maxfail || !buffers.is_empty() {
        let mut remaining: Vec<(usize, Option<PathBuf>)> = buffers
            .iter()
            .map(|(file, buf)| {
                let min_idx = buf.iter().map(|(idx, _)| *idx).min().unwrap_or(usize::MAX);
                (min_idx, file.clone())
            })
            .collect();
        remaining.sort_by_key(|(idx, _)| *idx);
        for (_, file) in remaining {
            flush_buffer(&file, &mut buffers, reporter);
        }
    }

    let summary = RunSummary {
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
    };
    reporter.on_run_complete(&summary);
    Ok(summary)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchKeyAction {
    Quit,
    RunAll,
    ClearResults,
    Ignore,
}

enum WatchLoopEvent {
    Command(WatchKeyAction),
    Files(FileChangeBatch),
    WatcherClosed,
}

fn watch_key_action(key: Key) -> WatchKeyAction {
    match key {
        Key::Char('q' | 'Q') | Key::Escape => WatchKeyAction::Quit,
        Key::Enter => WatchKeyAction::RunAll,
        Key::Char('c' | 'C') => WatchKeyAction::ClearResults,
        _ => WatchKeyAction::Ignore,
    }
}

/// Spawns a thread that forwards terminal commands to the async watch loop.
fn spawn_key_listener() -> tokio::sync::mpsc::UnboundedReceiver<WatchKeyAction> {
    use std::io::IsTerminal;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    if !std::io::stdin().is_terminal() {
        return rx;
    }

    let term = Term::stdout();

    std::thread::spawn(move || {
        while let Ok(key) = term.read_key() {
            let action = watch_key_action(key);
            match action {
                WatchKeyAction::Quit => {
                    let _ = tx.send(action);
                    break;
                }
                WatchKeyAction::RunAll | WatchKeyAction::ClearResults => {
                    let _ = tx.send(action);
                }
                WatchKeyAction::Ignore => continue,
            }
        }
    });

    rx
}

fn emit_discovery_warnings(reporter: &mut dyn Reporter, discoverer: &Discoverer) {
    for path in discoverer.dynamic_import_files() {
        let message = format!(
            "{} — dynamic imports found; will always re-run in watch mode",
            path.display()
        );
        reporter.on_discovery_warning(&DiscoveryWarning {
            file_path: path,
            kind: DiscoveryWarningKind::DynamicImports,
            message,
        });
    }
    for (path, line) in discoverer.testing_guard_else_locations() {
        let message = format!(
            "{}:{line} — `if __TRYKE_TESTING__:` has elif/else; tests inside will NOT be \
             discovered. Move production fallback code above or below the guard.",
            path.display()
        );
        reporter.on_discovery_warning(&DiscoveryWarning {
            file_path: path,
            kind: DiscoveryWarningKind::TestingGuardHasElseBranch,
            message,
        });
    }
}

fn clear_watch_results(reporter: &mut dyn Reporter) {
    reporter.on_watch_results_cleared(&WatchIdleInfo {
        hint: "Results cleared. Waiting for file changes...",
        start_time: None,
        discovery_duration: None,
    });
}

async fn run_watch_cycle(
    reporter: &mut dyn Reporter,
    tests: Vec<tryke_types::TestItem>,
    hooks: &[HookItem],
    pool: &WorkerPool,
    maxfail: Option<usize>,
    dist: DistMode,
    discovery_duration: Option<Duration>,
) {
    pool.restart_workers().await;

    if let Err(e) = report_cycle(
        reporter,
        tests,
        hooks,
        pool,
        maxfail,
        dist,
        discovery_duration,
        None,
    )
    .await
    {
        debug!("Watch: report_cycle errored: {e}");
    }
}

async fn run_initial_cycle(
    reporter: &mut dyn Reporter,
    discoverer: &mut Discoverer,
    test_filter: &TestFilter,
    pool: &WorkerPool,
    maxfail: Option<usize>,
    dist: DistMode,
    run_now: bool,
) {
    reporter.arm_clear();

    let disc_start = Instant::now();
    let initial_tests = discoverer.rediscover();
    let disc_dur = disc_start.elapsed();

    emit_discovery_warnings(reporter, discoverer);

    if run_now {
        let tests = test_filter.apply(initial_tests);
        let hooks = discoverer.hooks();

        run_watch_cycle(reporter, tests, &hooks, pool, maxfail, dist, Some(disc_dur)).await;
    } else {
        let start_time = chrono::Local::now().format("%H:%M:%S").to_string();

        reporter.on_watch_idle(&WatchIdleInfo {
            hint: "Waiting for file changes...",
            start_time: Some(&start_time),
            discovery_duration: Some(disc_dur),
        });
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Watch options map directly to CLI flags; grouping into a struct would add indirection without clear benefit."
)]
async fn run_watch(
    reporter: &mut dyn Reporter,
    project: &Project,
    log_level: LevelFilter,
    test_filter: &TestFilter,
    maxfail: Option<usize>,
    workers: usize,
    dist: DistMode,
    all_tests: bool,
    run_now: bool,
    cancellation: &CancellationToken,
) -> Result<Interruptible<()>> {
    let pool = WorkerPool::spawn(
        project,
        WorkerPoolOptions {
            size: workers,
            python_path: None,
            log_level,
            warm: false,
        },
    )
    .await;

    let run_result = run_interruptibly(
        run_watch_loop(
            reporter,
            project,
            test_filter,
            &pool,
            maxfail,
            dist,
            all_tests,
            run_now,
        ),
        cancellation,
    )
    .await;
    let shutdown_result = pool.shutdown().await;

    combine_shutdown(run_result, shutdown_result)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Watch options map directly to CLI flags; grouping into a struct would add indirection without clear benefit."
)]
async fn run_watch_loop(
    reporter: &mut dyn Reporter,
    project: &Project,
    test_filter: &TestFilter,
    pool: &WorkerPool,
    maxfail: Option<usize>,
    dist: DistMode,
    all_tests: bool,
    run_now: bool,
) -> Result<()> {
    let root = project.root();
    let excludes = &project.discovery().exclude;
    let mut discoverer = Discoverer::new(project);

    run_initial_cycle(
        reporter,
        &mut discoverer,
        test_filter,
        pool,
        maxfail,
        dist,
        run_now,
    )
    .await;

    let mut watcher = FileWatcher::spawn(root, excludes)?;
    let mut commands = spawn_key_listener();

    loop {
        let event = tokio::select! {
            batch = watcher.next_batch() => match batch? {
                Some(batch) => WatchLoopEvent::Files(batch),
                None => WatchLoopEvent::WatcherClosed,
            },
            Some(command) = commands.recv() => WatchLoopEvent::Command(command),
        };

        let paths = match event {
            WatchLoopEvent::Command(WatchKeyAction::Quit) | WatchLoopEvent::WatcherClosed => break,
            WatchLoopEvent::Command(WatchKeyAction::RunAll) => {
                watcher.discard_pending();
                reporter.arm_clear();
                let disc_start = Instant::now();
                discoverer.rediscover();
                let raw_tests = discoverer.tests();
                let tests = test_filter.apply(raw_tests);
                let hooks = discoverer.hooks();
                let disc_dur = Some(disc_start.elapsed());
                emit_discovery_warnings(reporter, &discoverer);
                run_watch_cycle(reporter, tests, &hooks, pool, maxfail, dist, disc_dur).await;
                continue;
            }
            WatchLoopEvent::Command(WatchKeyAction::ClearResults) => {
                clear_watch_results(reporter);
                continue;
            }
            WatchLoopEvent::Command(WatchKeyAction::Ignore) => continue,
            WatchLoopEvent::Files(batch) => batch.paths,
        };

        debug!(
            "Watch: file change batch — {} path(s) changed: {}",
            paths.len(),
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        reporter.arm_clear();

        let discovery_start = Instant::now();
        let change_impact = discoverer.apply_changes(&paths);
        let discovery_duration = Some(discovery_start.elapsed());

        if change_impact.paths.is_empty() {
            debug!("Watch: no eligible paths after discovery filtering");
            continue;
        }

        let raw_tests = if all_tests {
            discoverer.tests()
        } else {
            change_impact.affected_tests
        };

        let tests = test_filter.apply(raw_tests);
        let hooks = discoverer.hooks();

        emit_discovery_warnings(reporter, &discoverer);

        run_watch_cycle(
            reporter,
            tests,
            &hooks,
            pool,
            maxfail,
            dist,
            discovery_duration,
        )
        .await;
    }

    Ok(())
}

#[cfg(test)]
mod interrupt_tests {
    use std::future::ready;

    use super::*;

    #[derive(Default)]
    struct CleanupReporter {
        cleanup_calls: usize,
    }

    impl Reporter for CleanupReporter {
        fn on_run_start(&mut self, _tests: &[tryke_types::TestItem]) {}

        fn on_test_complete(&mut self, _result: &tryke_types::TestResult) {}

        fn on_run_complete(&mut self, _summary: &RunSummary) {}

        fn cleanup(&mut self) {
            self.cleanup_calls += 1;
        }
    }

    #[tokio::test]
    async fn interruption_takes_priority_and_cleans_up_reporter() -> Result<()> {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = run_interruptibly(ready(Ok::<_, anyhow::Error>(())), &cancellation).await;
        let mut reporter = CleanupReporter::default();

        assert!(resolve_interruptible(result, &mut reporter)?.is_none());
        assert_eq!(reporter.cleanup_calls, 1);
        Ok(())
    }

    #[tokio::test]
    async fn completion_does_not_clean_up_reporter() -> Result<()> {
        let cancellation = CancellationToken::new();
        let result = run_interruptibly(ready(Ok::<_, anyhow::Error>(42)), &cancellation).await;
        let mut reporter = CleanupReporter::default();

        assert_eq!(resolve_interruptible(result, &mut reporter)?, Some(42));
        assert_eq!(reporter.cleanup_calls, 0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tryke_reporter::{JSONReporter, TextReporter};

    use super::*;

    #[test]
    fn collect_only_text_reports_discovered_tests() {
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
    fn collect_only_json_reports_discovered_tests() {
        let mut reporter = JSONReporter::with_writer(Vec::new());
        let tests = tryke_discovery::discover().expect("current_dir");
        reporter.on_collect_complete(&tests);
        let buf = reporter.into_writer();
        let out = String::from_utf8_lossy(&buf);
        let value: serde_json::Value = serde_json::from_str(out.trim()).expect("valid json");
        assert_eq!(value["event"], "collect_complete");
        assert!(value["tests"].is_array());
    }
}

#[cfg(test)]
mod watch_tests {
    use std::{io, path::PathBuf};

    use tryke_reporter::TextReporter;
    use tryke_testing::{TestProject, python_bin as test_python_bin};

    use super::*;

    #[tokio::test]
    async fn run_watch_cycle_absorbs_test_failures() -> io::Result<()> {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let fixture = TestProject::with_files([(
            "test_fail.py",
            "from tryke import test, expect\n\n@test\ndef test_bad():\n    expect(1 + 1).to_equal(3)\n",
        )])?;
        let project = fixture.project();
        let tests = Discoverer::new(&project)
            .discover(DiscoveryOptions::default())
            .tests;
        let mut reporter = TextReporter::with_writer(Vec::new());
        let python_path = [fixture.root().to_path_buf(), python_dir];
        let pool = WorkerPool::spawn_from_parts(
            1,
            &test_python_bin(),
            fixture.root(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;
        // Returns () — the important behavior is that it does NOT propagate the
        // underlying `report_cycle` Err that `tryke test` relies on for exit code.
        run_watch_cycle(&mut reporter, tests, &[], &pool, None, DistMode::Test, None).await;
        pool.shutdown().await.expect("shut down worker pool");
        Ok(())
    }

    /// Reporter that just tallies how many runs / tests it saw, so the
    /// `run_now` gating can be asserted without parsing reporter output.
    #[derive(Default)]
    struct CountingReporter {
        run_starts: usize,
        test_completes: usize,
        run_completes: usize,
    }

    impl Reporter for CountingReporter {
        fn on_run_start(&mut self, _tests: &[tryke_types::TestItem]) {
            self.run_starts += 1;
        }
        fn on_test_complete(&mut self, _result: &tryke_types::TestResult) {
            self.test_completes += 1;
        }
        fn on_run_complete(&mut self, _summary: &tryke_types::RunSummary) {
            self.run_completes += 1;
        }
    }

    async fn run_initial_cycle_for_test(run_now: bool) -> io::Result<CountingReporter> {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let fixture = TestProject::with_files([(
            "test_ok.py",
            "from tryke import test, expect\n\n@test\ndef test_ok():\n    expect(1).to_equal(1)\n",
        )])?;
        let src_roots = fixture.project().src_roots();
        let mut discoverer = Discoverer::from_parts(fixture.root(), src_roots, &[], None);
        let test_filter = TestFilter::from_args(&[], None, None).expect("filter");
        let python_path = [fixture.root().to_path_buf(), python_dir];
        let pool = WorkerPool::spawn_from_parts(
            1,
            &test_python_bin(),
            fixture.root(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;
        let mut reporter = CountingReporter::default();
        run_initial_cycle(
            &mut reporter,
            &mut discoverer,
            &test_filter,
            &pool,
            None,
            DistMode::Test,
            run_now,
        )
        .await;
        pool.shutdown().await.expect("shut down worker pool");
        Ok(reporter)
    }

    #[tokio::test]
    async fn run_initial_cycle_skips_tests_when_run_now_is_false() -> io::Result<()> {
        let reporter = run_initial_cycle_for_test(false).await?;
        assert_eq!(reporter.run_starts, 0, "no run should fire on idle startup");
        assert_eq!(reporter.test_completes, 0);
        assert_eq!(reporter.run_completes, 0);
        Ok(())
    }

    #[tokio::test]
    async fn run_initial_cycle_runs_tests_when_run_now_is_true() -> io::Result<()> {
        let reporter = run_initial_cycle_for_test(true).await?;
        assert_eq!(reporter.run_starts, 1, "exactly one initial run expected");
        assert_eq!(reporter.test_completes, 1);
        assert_eq!(reporter.run_completes, 1);
        Ok(())
    }

    #[test]
    fn watch_keys_map_to_actions() {
        assert_eq!(watch_key_action(Key::Char('q')), WatchKeyAction::Quit);
        assert_eq!(watch_key_action(Key::Char('Q')), WatchKeyAction::Quit);
        assert_eq!(watch_key_action(Key::Escape), WatchKeyAction::Quit);
        assert_eq!(watch_key_action(Key::Enter), WatchKeyAction::RunAll);
        assert_eq!(
            watch_key_action(Key::Char('c')),
            WatchKeyAction::ClearResults
        );
        assert_eq!(
            watch_key_action(Key::Char('C')),
            WatchKeyAction::ClearResults
        );
        assert_eq!(watch_key_action(Key::Char('x')), WatchKeyAction::Ignore);
    }
}

#[cfg(test)]
mod execution_tests {
    use std::{
        io,
        path::PathBuf,
        sync::Arc,
        time::{Duration, Instant},
    };

    use anyhow::Context as _;
    use tokio::sync::Notify;
    use tryke_config::{Project, ProjectMetadata, TrykeOptions};
    use tryke_discovery::{Discoverer, DiscoveryOptions};
    use tryke_reporter::{
        DotReporter, JSONReporter, JUnitReporter, NextReporter, SugarReporter, TextReporter,
    };
    use tryke_testing::{TestProject, python_bin as test_python_bin};

    use super::*;

    struct CancellationReporter {
        started: Arc<Notify>,
        run_completes: usize,
        cleanup_calls: usize,
    }

    impl Reporter for CancellationReporter {
        fn on_run_start(&mut self, _tests: &[tryke_types::TestItem]) {
            self.started.notify_one();
        }

        fn on_test_complete(&mut self, _result: &tryke_types::TestResult) {}

        fn on_run_complete(&mut self, _summary: &RunSummary) {
            self.run_completes += 1;
        }

        fn cleanup(&mut self) {
            self.cleanup_calls += 1;
        }
    }

    fn configured_project(root: &std::path::Path) -> Project {
        let mut metadata = ProjectMetadata::new(root);
        metadata.apply_configuration_file();
        metadata.apply_cli_args(TrykeOptions {
            python: Some(test_python_bin()),
            ..TrykeOptions::default()
        });
        Project::from_metadata(metadata)
    }

    fn discover_project(
        project: &Project,
        options: DiscoveryOptions<'_>,
    ) -> Vec<tryke_types::TestItem> {
        Discoverer::new(project).discover(options).tests
    }

    async fn run_cycle(
        reporter: &mut dyn Reporter,
        discoverer: &mut Discoverer,
        pool: &WorkerPool,
    ) -> anyhow::Result<RunSummary> {
        report_cycle(
            reporter,
            discoverer.rediscover(),
            &[],
            pool,
            None,
            DistMode::Test,
            None,
            None,
        )
        .await
    }

    /// Smoke-test a reporter against the full `run_tests` pipeline using an
    /// empty project. Exercises pool init/teardown and the reporter's
    /// run_start/run_summary callbacks without doing real work. Snapshot
    /// tests in `tests/snapshots.rs` cover per-test rendering.
    async fn smoke_run_tests(reporter: &mut dyn Reporter) -> io::Result<()> {
        let fixture = TestProject::new()?;
        let project = configured_project(fixture.root());
        let tests = discover_project(&project, DiscoveryOptions::default());
        let cancellation = CancellationToken::new();
        let _ = run_tests(
            reporter,
            &project,
            LevelFilter::Off,
            tests,
            &[],
            None,
            1,
            DistMode::Test,
            None,
            None,
            &cancellation,
        )
        .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_command_text() -> io::Result<()> {
        let mut reporter = TextReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await
    }

    #[tokio::test]
    async fn test_command_json() -> io::Result<()> {
        let mut reporter = JSONReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await
    }

    #[tokio::test]
    async fn test_command_dot() -> io::Result<()> {
        let mut reporter = DotReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await
    }

    #[tokio::test]
    async fn test_command_junit() -> io::Result<()> {
        let mut reporter = JUnitReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await
    }

    #[tokio::test]
    async fn test_command_next() -> io::Result<()> {
        let mut reporter = NextReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await
    }

    #[tokio::test]
    async fn test_command_sugar() -> io::Result<()> {
        let mut reporter = SugarReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await
    }

    #[tokio::test]
    async fn run_cycle_runs_without_error() -> io::Result<()> {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let fixture = TestProject::with_files([(
            "test_x.py",
            "from tryke import test\n\n@test\ndef test_x(): pass\n",
        )])?;
        let src_roots = tryke_config::DiscoveryConfig::default().src_roots(fixture.root());
        let mut discoverer = Discoverer::from_parts(fixture.root(), src_roots, &[], None);
        let mut reporter = TextReporter::new();
        let python_path = [fixture.root().to_path_buf(), python_dir];
        let pool = WorkerPool::spawn_from_parts(
            1,
            &test_python_bin(),
            fixture.root(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;
        assert!(
            run_cycle(&mut reporter, &mut discoverer, &pool)
                .await
                .is_ok()
        );
        pool.shutdown().await.expect("shut down worker pool");
        Ok(())
    }

    #[tokio::test]
    async fn run_cycle_with_json_reporter() -> io::Result<()> {
        let fixture = TestProject::new()?;
        let src_roots = tryke_config::DiscoveryConfig::default().src_roots(fixture.root());
        let mut discoverer = Discoverer::from_parts(fixture.root(), src_roots, &[], None);
        let mut reporter = JSONReporter::with_writer(Vec::new());
        let pool = WorkerPool::spawn_from_parts(
            1,
            &test_python_bin(),
            fixture.root(),
            None,
            LevelFilter::Off,
            false,
        )
        .await;
        assert!(
            run_cycle(&mut reporter, &mut discoverer, &pool)
                .await
                .is_ok()
        );
        pool.shutdown().await.expect("shut down worker pool");
        Ok(())
    }

    #[tokio::test]
    async fn run_changed_test_without_git_runs_all() -> io::Result<()> {
        let fixture = TestProject::new()?;
        let mut reporter = TextReporter::new();
        // Non-git directory means changed discovery falls back to all tests.
        let project = configured_project(fixture.root());
        let tests = discover_project(
            &project,
            DiscoveryOptions {
                changed: true,
                ..DiscoveryOptions::default()
            },
        );
        let cancellation = CancellationToken::new();
        assert!(
            run_tests(
                &mut reporter,
                &project,
                LevelFilter::Off,
                tests,
                &[],
                None,
                1,
                DistMode::Test,
                None,
                None,
                &cancellation,
            )
            .await
            .is_ok()
        );
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_interrupts_active_test_and_awaits_shutdown() -> anyhow::Result<()> {
        let fixture = TestProject::with_files([(
            "test_sleep.py",
            "import time\nfrom tryke import test\n\n@test\ndef test_sleep():\n    time.sleep(30)\n",
        )])?;
        let project = configured_project(fixture.root());
        let mut discoverer = Discoverer::new(&project);
        let tests = discoverer.rediscover();
        let hooks = discoverer.hooks();

        let started = Arc::new(Notify::new());
        let mut reporter = CancellationReporter {
            started: Arc::clone(&started),
            run_completes: 0,
            cleanup_calls: 0,
        };
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        let cancel_task = tokio::spawn(async move {
            started.notified().await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });

        let start = Instant::now();
        let result = run_tests(
            &mut reporter,
            &project,
            LevelFilter::Off,
            tests,
            &hooks,
            None,
            1,
            DistMode::Test,
            None,
            None,
            &cancellation,
        )
        .await;
        cancel_task.await.context("Join cancellation task")?;

        assert!(resolve_interruptible(result, &mut reporter)?.is_none());
        assert_eq!(reporter.cleanup_calls, 1);
        assert_eq!(reporter.run_completes, 0);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "Cancellation should not wait for the sleeping Python test",
        );
        Ok(())
    }

    #[tokio::test]
    async fn integration_python_worker_runs_tests() -> io::Result<()> {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        let python_dir = workspace_root.join("python");

        let fixture = TestProject::with_files([(
            "test_example.py",
            "\
from tryke import test, expect

@test
def test_passing():
    expect(1 + 1).to_equal(2)

@test
def test_failing():
    expect(1 + 1).to_equal(3)
",
        )])?;

        let project = configured_project(fixture.root());
        let tests = discover_project(&project, DiscoveryOptions::default());
        assert_eq!(tests.len(), 2);

        let python_path = [fixture.root().to_path_buf(), python_dir];
        let pool = WorkerPool::spawn_from_parts(
            1,
            &test_python_bin(),
            fixture.root(),
            Some(&python_path),
            LevelFilter::Off,
            true,
        )
        .await;
        let units = partition_with_hooks(tests, &[], DistMode::Test).units;
        let mut results: Vec<_> = pool.submit(units).collect().await;
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

        pool.shutdown().await.expect("shut down worker pool");
        Ok(())
    }

    #[tokio::test]
    async fn report_cycle_returns_ok_when_all_pass() -> io::Result<()> {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let fixture = TestProject::with_files([(
            "test_pass.py",
            "from tryke import test, expect\n\n@test\ndef test_ok():\n    expect(1 + 1).to_equal(2)\n",
        )])?;
        let project = configured_project(fixture.root());
        let tests = discover_project(&project, DiscoveryOptions::default());
        let mut reporter = TextReporter::with_writer(Vec::new());
        let python_path = [fixture.root().to_path_buf(), python_dir];
        let pool = WorkerPool::spawn_from_parts(
            1,
            &test_python_bin(),
            fixture.root(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;
        let result = report_cycle(
            &mut reporter,
            tests,
            &[],
            &pool,
            None,
            DistMode::Test,
            None,
            None,
        )
        .await;
        assert!(
            result.is_ok(),
            "expected Ok when all tests pass, got {result:?}"
        );
        pool.shutdown().await.expect("shut down worker pool");
        Ok(())
    }

    #[tokio::test]
    async fn report_cycle_summary_reports_failures() -> io::Result<()> {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let fixture = TestProject::with_files([(
            "test_fail.py",
            "from tryke import test, expect\n\n@test\ndef test_bad():\n    expect(1 + 1).to_equal(3)\n",
        )])?;
        let project = configured_project(fixture.root());
        let tests = discover_project(&project, DiscoveryOptions::default());
        let mut reporter = TextReporter::with_writer(Vec::new());
        let python_path = [fixture.root().to_path_buf(), python_dir];
        let pool = WorkerPool::spawn_from_parts(
            1,
            &test_python_bin(),
            fixture.root(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;
        let summary = report_cycle(
            &mut reporter,
            tests,
            &[],
            &pool,
            None,
            DistMode::Test,
            None,
            None,
        )
        .await
        .expect("report_cycle should not error on test failures");
        assert_eq!(summary.failed, 1, "expected one failed test");
        assert_eq!(summary.passed, 0);
        pool.shutdown().await.expect("shut down worker pool");
        Ok(())
    }
}
