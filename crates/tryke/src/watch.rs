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
use tryke_reporter::Reporter;
use tryke_runner::{DistMode, WorkerPool};
use tryke_types::{DiscoveryWarning, DiscoveryWarningKind, HookItem, filter::TestFilter};

use crate::execution::{report_cycle, worker_pool_size};

/// How often the watch loop wakes up to check the quit flag while
/// waiting for file events. Short enough that `q` feels responsive,
/// long enough to avoid burning CPU on a tight poll.
const QUIT_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Spawn a thread that reads single keypresses from stdin and flips
/// `quit` to `true` when the user presses `q` (or `Q`, or Escape).
/// No-op when stdin isn't a TTY (CI, piped input).
fn spawn_quit_listener(quit: Arc<AtomicBool>) {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return;
    }
    let term = Term::stdout();
    std::thread::spawn(move || {
        loop {
            match term.read_key() {
                Ok(Key::Char('q' | 'Q') | Key::Escape) => {
                    quit.store(true, Ordering::SeqCst);
                    break;
                }
                Err(_) => break,
                Ok(_) => continue,
            }
        }
    });
}

fn clear_if_tty() {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        let _ = clearscreen::clear();
    }
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
/// change. When `run_now` is false we leave the user's terminal
/// untouched (no `clear_if_tty`) and print a single idle hint so it's
/// obvious the watcher is alive.
async fn run_initial_cycle(
    reporter: &mut dyn Reporter,
    discoverer: &mut Discoverer,
    test_filter: &TestFilter,
    pool: &WorkerPool,
    maxfail: Option<usize>,
    dist: DistMode,
    run_now: bool,
) {
    let disc_start = Instant::now();
    let initial_tests = discoverer.rediscover();
    let disc_dur = disc_start.elapsed();
    emit_discovery_warnings(reporter, discoverer);
    if run_now {
        clear_if_tty();
        let tests = test_filter.apply(initial_tests);
        let hooks = discoverer.hooks();
        run_watch_cycle(reporter, tests, &hooks, pool, maxfail, dist, Some(disc_dur)).await;
    } else {
        use std::io::IsTerminal;
        if std::io::stderr().is_terminal() {
            eprintln!("tryke watch: idle — waiting for file changes... press q to quit");
        }
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

    let quit = Arc::new(AtomicBool::new(false));
    spawn_quit_listener(Arc::clone(&quit));

    loop {
        if quit.load(Ordering::SeqCst) {
            break;
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
        discoverer.rediscover_changed(&paths);
        clear_if_tty();
        let disc_start = Instant::now();
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
