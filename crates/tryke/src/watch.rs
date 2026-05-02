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
use log::debug;
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

/// Run a single watch cycle. Test failures are non-fatal here: `report_cycle`
/// returns `Err` purely to signal pass/fail state to `tryke test`, but in watch
/// mode the whole point is to iterate on failing tests — so we absorb it and
/// let the watcher keep running.
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
        debug!("watch: test cycle reported failures: {e}");
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
    excludes: &[String],
    test_filter: &TestFilter,
    maxfail: Option<usize>,
    workers: Option<usize>,
    dist: DistMode,
    all_tests: bool,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new_with_excludes(root, excludes);

    let pool_size = workers.unwrap_or_else(worker_pool_size);
    let pool = WorkerPool::new(pool_size, python, root);
    pool.warm().await;

    clear_if_tty();
    let disc_start = Instant::now();
    let tests = test_filter.apply(discoverer.rediscover());
    let hooks = discoverer.hooks();
    let disc_dur = Some(disc_start.elapsed());
    emit_discovery_warnings(reporter, &discoverer);
    run_watch_cycle(reporter, tests, &hooks, &pool, maxfail, dist, disc_dur).await;

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

    use super::*;
    use crate::discovery::discover_tests;

    fn test_python_bin() -> String {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (venv, fallback) = if cfg!(windows) {
            (workspace.join(".venv/Scripts/python.exe"), "python")
        } else {
            (workspace.join(".venv/bin/python3"), "python3")
        };
        if venv.exists() {
            venv.to_string_lossy().into_owned()
        } else {
            fallback.to_owned()
        }
    }

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
        );
        // Returns () — the important behavior is that it does NOT propagate the
        // underlying `report_cycle` Err that `tryke test` relies on for exit code.
        run_watch_cycle(&mut reporter, tests, &[], &pool, None, DistMode::Test, None).await;
        pool.shutdown();
    }
}
