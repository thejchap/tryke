use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::RecvTimeoutError,
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use console::{Key, Term};
use log::{LevelFilter, debug};
use tryke_discovery::Discoverer;
use tryke_reporter::{Reporter, reporter::WatchIdleInfo};
use tryke_runner::{DistMode, WorkerPool};
use tryke_types::{DiscoveryWarning, DiscoveryWarningKind, HookItem, filter::TestFilter};

use crate::execution::{ReportCycleRequest, report_cycle, worker_pool_size};

/// How often the watch loop wakes up to check the quit flag while
/// waiting for file events. Short enough that `q` feels responsive,
/// long enough to avoid burning CPU on a tight poll.
const QUIT_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Atomic flags driven by the keyboard listener thread; the watch
/// loop polls these alongside the file-change channel.
#[derive(Default)]
struct WatchKeys {
    quit: AtomicBool,
    run_all: AtomicBool,
}

/// Spawn a thread that reads single keypresses from stdin and flips
/// `keys` accordingly: `q`/`Q`/Escape sets `quit`; Enter sets
/// `run_all` (consumed once by the watch loop to trigger an explicit
/// full-suite run). No-op when stdin isn't a TTY (CI, piped input).
fn spawn_key_listener(keys: Arc<WatchKeys>) {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return;
    }
    let term = Term::stdout();
    std::thread::spawn(move || {
        loop {
            match term.read_key() {
                Ok(Key::Char('q' | 'Q') | Key::Escape) => {
                    keys.quit.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(Key::Enter) => {
                    keys.run_all.store(true, Ordering::SeqCst);
                }
                Err(_) => break,
                Ok(_) => continue,
            }
        }
    });
}

fn emit_discovery_warnings(reporter: &mut dyn Reporter, discoverer: &Discoverer) {
    for path in discoverer.dynamic_import_files() {
        let message = format!(
            "{} — dynamic imports found; will always re-run and may serve stale module state in watch mode",
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
    if let Err(e) = report_cycle(
        reporter,
        ReportCycleRequest {
            tests,
            hooks,
            pool,
            maxfail,
            dist,
            discovery_duration,
            changed_selection: None,
        },
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
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new_with_excludes(root, excludes);

    let pool_size = workers.unwrap_or_else(worker_pool_size);
    let pool = WorkerPool::new(pool_size, python, root, log_level);
    pool.warm().await;

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

    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let _debouncer = tryke_server::watcher::spawn_watcher(root, excludes, tx)?;
    let mut change_filter = tryke_server::watcher::ChangeFilter::new();

    let keys = Arc::new(WatchKeys::default());
    spawn_key_listener(Arc::clone(&keys));

    loop {
        if keys.quit.load(Ordering::SeqCst) {
            break;
        }
        // Explicit "run all tests" beats any queued file events:
        // drain the channel so we don't fire a duplicate cycle right
        // after, then run the full discovered set against fresh
        // workers.
        if keys.run_all.swap(false, Ordering::SeqCst) {
            while rx.try_recv().is_ok() {}
            pool.restart_workers().await;
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
        let first = match rx.recv_timeout(QUIT_POLL_INTERVAL) {
            Ok(paths) => paths,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // Coalesce any additional batches the watcher has already
        // queued. A single editor save can produce events whose
        // intermediate quiet windows fall just outside the watcher's
        // debounce — so two batches arrive back-to-back. Without this
        // drain, each batch triggers its own restart + test cycle,
        // which the user perceives as a double-restart.
        let mut paths = first;
        while let Ok(more) = rx.try_recv() {
            paths.extend(more);
        }
        paths.sort();
        paths.dedup();

        // Drop paths whose `(mtime, size)` is unchanged since the
        // last batch we accepted. This is the deterministic answer
        // to "did the file actually change" — drain only catches
        // batches queued together; this catches tail events that
        // arrive after the previous cycle has finished.
        let paths = change_filter.filter(&paths);
        if paths.is_empty() {
            debug!("watch: file change batch had no real content changes — skipping");
            continue;
        }

        debug!(
            "watch: file change batch — {} path(s) changed: {}",
            paths.len(),
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let modules = discoverer.affected_modules(&paths);
        if modules.is_empty() {
            debug!("watch: no modules affected by change — skipping worker restart");
        } else {
            debug!(
                "watch: restarting worker pool to pick up changes in {} module(s): {}",
                modules.len(),
                modules.join(", ")
            );
            pool.restart_workers().await;
        }
        // Arm before the heavy rediscover so the previous cycle's
        // output stays on screen while discovery + worker warmup
        // happens. The reporter clears at the moment new content is
        // about to land (warning, error, or run start), eliminating
        // the blank-screen gap that's painful on large suites.
        reporter.arm_clear();
        // Time the full discovery work — `rediscover_changed` is the
        // expensive part on large suites, so it has to be inside the
        // measured window for `disc_dur` to mean anything.
        let disc_start = Instant::now();
        discoverer.rediscover_changed(&paths);
        // When `--all` is set, rerun the full test set on every change instead
        // of restricting to tests transitively affected by the changed files.
        // Useful when the import graph misses dependencies (dynamic imports,
        // string-referenced modules, external fixtures) or when debugging
        // test ordering/flakiness.
        let raw_tests = if all_tests {
            discoverer.tests()
        } else {
            discoverer.tests_for_changed(&paths)
        };
        let tests = test_filter.apply(raw_tests);
        let hooks = discoverer.hooks();
        let disc_dur = Some(disc_start.elapsed());
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
        let tests = discover_tests(dir.path(), false, None, &[]).tests;
        let mut reporter = TextReporter::with_writer(Vec::new());
        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_dir],
            LevelFilter::Off,
        );
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
        let mut discoverer = Discoverer::new_with_excludes(dir.path(), &[]);
        let test_filter = TestFilter::from_args(&[], None, None).expect("filter");
        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_dir],
            LevelFilter::Off,
        );
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
}
