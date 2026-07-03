use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Result;
use console::{Key, Term};
use log::{LevelFilter, debug};
use tryke_config::load_effective_config;
use tryke_discovery::{Discoverer, resolve_project_root};
use tryke_reporter::{Reporter, reporter::WatchIdleInfo};
use tryke_runner::{DistMode, WorkerPool};
use tryke_types::{DiscoveryWarning, DiscoveryWarningKind, HookItem, filter::TestFilter};
use tryke_watcher::{FileChangeBatch, FileWatcher};

use crate::execution::{report_cycle, worker_pool_size};

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

/// Run a single watch cycle. Test failures are non-fatal here: in watch
/// mode the whole point is to iterate on failing tests, so we discard
/// the run summary and any setup error and let the watcher keep running.
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
        debug!("watch: report_cycle errored: {e}");
    }
}

/// Perform startup discovery and, when `run_now` is set, the initial
/// test cycle. Discovery runs unconditionally so the import graph is
/// ready to answer "which tests are affected?" on the first file
/// change. When `run_now` is false we hand the reporter an idle frame
/// (header + Tests/Start/Discovery block + IDLE badge) so the
/// terminal communicates clearly that the watcher is alive and
/// waiting.
async fn run_initial_cycle(
    reporter: &mut dyn Reporter,
    discoverer: &mut Discoverer,
    test_filter: &TestFilter,
    pool: &WorkerPool,
    maxfail: Option<usize>,
    dist: DistMode,
    run_now: bool,
) {
    // Arm before any reporter output so the deferred clear lands on
    // the first warning, run-start, or idle frame — whichever fires
    // first. The reporter's `flush_pending_clear` (called from each
    // of those paths) consumes the flag, so warnings emitted just
    // before `on_watch_idle` aren't wiped by a second clear inside
    // the idle render.
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
pub async fn run_watch(
    reporter: &mut dyn Reporter,
    root: Option<&Path>,
    python: &str,
    log_level: LevelFilter,
    excludes: &[String],
    test_filter: &TestFilter,
    maxfail: Option<usize>,
    workers: Option<usize>,
    dist: DistMode,
    all_tests: bool,
    run_now: bool,
    cache_dir: Option<&Path>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = resolve_project_root(root.unwrap_or(&cwd));
    let src_roots = load_effective_config(&root).discovery.src_roots(&root);
    let mut discoverer = Discoverer::new(&root, src_roots, excludes, cache_dir);

    let pool_size = workers.unwrap_or_else(worker_pool_size);
    let pool = WorkerPool::spawn(pool_size, python, &root, None, log_level, false).await;

    run_initial_cycle(
        reporter,
        &mut discoverer,
        test_filter,
        &pool,
        maxfail,
        dist,
        run_now,
    )
    .await;

    let mut watcher = FileWatcher::spawn(&root, excludes)?;
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
                run_watch_cycle(reporter, tests, &hooks, &pool, maxfail, dist, disc_dur).await;
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
            "watch: file change batch — {} path(s) changed: {}",
            paths.len(),
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        // Arm before the heavy rediscover so the previous cycle's
        // output stays on screen while discovery + worker warmup
        // happens. The reporter clears at the moment new content is
        // about to land (warning, error, or run start), eliminating
        // the blank-screen gap that's painful on large suites.
        reporter.arm_clear();
        // Time the full discovery work — `apply_changes` is the
        // expensive part on large suites, so it has to be inside the
        // measured window for `disc_dur` to mean anything.
        let disc_start = Instant::now();
        let impact = discoverer.apply_changes(&paths);
        let disc_dur = Some(disc_start.elapsed());
        if impact.paths.is_empty() {
            debug!("watch: no eligible paths after discovery filtering");
            continue;
        }
        // When `--all` is set, rerun the full test set on every change instead
        // of restricting to tests transitively affected by the changed files.
        // Useful when the import graph misses dependencies (dynamic imports,
        // string-referenced modules, external fixtures) or when debugging
        // test ordering/flakiness.
        let raw_tests = if all_tests {
            discoverer.tests()
        } else {
            impact.affected_tests
        };
        let tests = test_filter.apply(raw_tests);
        let hooks = discoverer.hooks();
        emit_discovery_warnings(reporter, &discoverer);
        run_watch_cycle(reporter, tests, &hooks, &pool, maxfail, dist, disc_dur).await;
    }

    pool.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tryke_reporter::TextReporter;
    use tryke_testing::python_bin as test_python_bin;

    use super::*;
    use crate::discovery::discover_tests;

    #[tokio::test]
    async fn run_watch_cycle_absorbs_test_failures() {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_fail.py"),
            "from tryke import test, expect\n\n@test\ndef test_bad():\n    expect(1 + 1).to_equal(3)\n",
        )
        .expect("write test file");
        let tests = discover_tests(dir.path(), false, None, &[], None).tests;
        let mut reporter = TextReporter::with_writer(Vec::new());
        let python_path = [dir.path().to_path_buf(), python_dir];
        let pool = WorkerPool::spawn(
            1,
            &test_python_bin(),
            dir.path(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;
        // Returns () — the important behavior is that it does NOT propagate the
        // underlying `report_cycle` Err that `tryke test` relies on for exit code.
        run_watch_cycle(&mut reporter, tests, &[], &pool, None, DistMode::Test, None).await;
        pool.shutdown();
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

    async fn run_initial_cycle_for_test(run_now: bool) -> CountingReporter {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_ok.py"),
            "from tryke import test, expect\n\n@test\ndef test_ok():\n    expect(1).to_equal(1)\n",
        )
        .expect("write test file");
        let src_roots = load_effective_config(dir.path())
            .discovery
            .src_roots(dir.path());
        let mut discoverer = Discoverer::new(dir.path(), src_roots, &[], None);
        let test_filter = TestFilter::from_args(&[], None, None).expect("filter");
        let python_path = [dir.path().to_path_buf(), python_dir];
        let pool = WorkerPool::spawn(
            1,
            &test_python_bin(),
            dir.path(),
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
        pool.shutdown();
        reporter
    }

    #[tokio::test]
    async fn run_initial_cycle_skips_tests_when_run_now_is_false() {
        let reporter = run_initial_cycle_for_test(false).await;
        assert_eq!(reporter.run_starts, 0, "no run should fire on idle startup");
        assert_eq!(reporter.test_completes, 0);
        assert_eq!(reporter.run_completes, 0);
    }

    #[tokio::test]
    async fn run_initial_cycle_runs_tests_when_run_now_is_true() {
        let reporter = run_initial_cycle_for_test(true).await;
        assert_eq!(reporter.run_starts, 1, "exactly one initial run expected");
        assert_eq!(reporter.test_completes, 1);
        assert_eq!(reporter.run_completes, 1);
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
