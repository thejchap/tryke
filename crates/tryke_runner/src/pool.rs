use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use log::{LevelFilter, debug, trace};

use tokio::sync::{mpsc, oneshot};
use tokio_stream::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tryke_types::{HookItem, TestOutcome, TestResult};

use crate::protocol::RegisterHooksParams;
use crate::schedule::WorkUnit;
use crate::worker::{WorkerLimits, WorkerProcess};

/// Bundle of inputs every spawn (and respawn) needs. These five values
/// never vary across the lifetime of a `worker_task`, so threading them
/// individually through every helper just inflates the signature and
/// trips `clippy::too_many_arguments`. Grouping them keeps the call
/// sites short and lets us add new spawn-time knobs later without
/// touching every helper signature.
struct WorkerSpawnCtx<'a> {
    python_bin: &'a str,
    path_refs: &'a [&'a Path],
    root: &'a Path,
    log_level: LevelFilter,
    limits: WorkerLimits,
}

/// Per-worker-task state: the (optional) live Python process plus a cache of
/// the most recent `register_hooks` call per module. The cache exists so
/// that a freshly-spawned worker (after a crash) can be brought back to the
/// same hook-registration state as the one it replaces — otherwise
/// subsequent tests in the same unit would silently run without their
/// `before_each` / `after_each` fixtures.
struct WorkerState {
    process: Option<WorkerProcess>,
    hook_cache: HashMap<String, RegisterHooksParams>,
}

impl WorkerState {
    fn new() -> Self {
        Self {
            process: None,
            hook_cache: HashMap::new(),
        }
    }
}

enum WorkerMsg {
    Unit(WorkUnit, mpsc::UnboundedSender<TestResult>),
    Shutdown,
}

/// Control messages delivered on a per-worker channel.
///
/// `Ping` and `Restart` are fan-out operations: every worker must
/// receive exactly one. Routing them through the shared work-stealing
/// channel would let a single fast worker grab all N messages while
/// other workers remained on stale Python processes — defeating the
/// guarantee that watch/server-mode reloads run on a fresh interpreter.
/// A dedicated channel per worker eliminates that race.
enum WorkerCtrl {
    Ping(oneshot::Sender<()>),
    Restart(oneshot::Sender<()>),
}

pub struct WorkerPool {
    work_tx: async_channel::Sender<WorkerMsg>,
    ctrl_txs: Vec<mpsc::UnboundedSender<WorkerCtrl>>,
}

impl WorkerPool {
    /// Build a pool whose workers receive `TRYKE_LOG=<log_level>` on spawn.
    ///
    /// Pass `LevelFilter::Off` to leave workers silent (the env var is
    /// then not set on the child, so the worker's
    /// `_configure_logging_from_env` no-ops). Production callers use
    /// `tryke_config::worker_log_level` to derive this from CLI flags
    /// + the `TRYKE_LOG` env var.
    #[must_use]
    pub fn new(size: usize, python_bin: &str, root: &Path, log_level: LevelFilter) -> Self {
        let mut python_path = vec![root.to_path_buf()];
        // pyproject.toml declares python-source = "python" — add it to
        // PYTHONPATH so the tryke package is importable even without a venv.
        let src_dir = root.join("python");
        if src_dir.is_dir() {
            python_path.push(src_dir);
        }
        Self::with_python_path(size, python_bin, root, &python_path, log_level)
    }

    #[must_use]
    pub fn with_python_path(
        size: usize,
        python_bin: &str,
        root: &Path,
        python_path: &[PathBuf],
        log_level: LevelFilter,
    ) -> Self {
        Self::with_python_path_and_limits(
            size,
            python_bin,
            root,
            python_path,
            log_level,
            WorkerLimits::default(),
        )
    }

    /// Like [`Self::with_python_path`] but with explicit recycle
    /// thresholds. Tests use this to set tiny ceilings (or
    /// [`WorkerLimits::unlimited`]) so they can observe — or suppress —
    /// recycle behaviour without spinning up a real workload.
    #[must_use]
    pub fn with_python_path_and_limits(
        size: usize,
        python_bin: &str,
        root: &Path,
        python_path: &[PathBuf],
        log_level: LevelFilter,
        limits: WorkerLimits,
    ) -> Self {
        let size = size.max(1);
        let python_path = python_path.to_vec();
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let (work_tx, work_rx) = async_channel::unbounded();
        let mut ctrl_txs = Vec::with_capacity(size);
        for _ in 0..size {
            let bin = python_bin.to_owned();
            let work_rx = work_rx.clone();
            let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
            ctrl_txs.push(ctrl_tx);
            tokio::spawn(worker_task(
                bin,
                python_path.clone(),
                root.clone(),
                log_level,
                limits,
                work_rx,
                ctrl_rx,
            ));
        }
        Self { work_tx, ctrl_txs }
    }

    pub fn run(&self, units: Vec<WorkUnit>) -> impl Stream<Item = TestResult> + use<> {
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();

        for unit in units {
            let _ = self
                .work_tx
                .send_blocking(WorkerMsg::Unit(unit, stream_tx.clone()));
        }

        UnboundedReceiverStream::new(stream_rx)
    }

    /// Send one ctrl message per worker and await every ack.
    ///
    /// `build` is the ctrl-variant constructor (e.g. `WorkerCtrl::Ping`)
    /// — taking it as a function pointer lets `warm` and
    /// `restart_workers` share this whole fan-out path.
    ///
    /// If a worker task has died (ctrl channel receiver dropped), its
    /// `send` returns `Err`; we skip its ack rather than push a future
    /// that will never resolve, which would hang the watcher/server.
    async fn fanout_ctrl(&self, build: fn(oneshot::Sender<()>) -> WorkerCtrl) {
        let mut ack_rxs = Vec::with_capacity(self.ctrl_txs.len());
        for ctrl_tx in &self.ctrl_txs {
            let (ack_tx, ack_rx) = oneshot::channel();
            if ctrl_tx.send(build(ack_tx)).is_ok() {
                ack_rxs.push(ack_rx);
            }
        }
        for ack_rx in ack_rxs {
            let _ = ack_rx.await;
        }
    }

    /// Kill every worker subprocess and respawn a fresh one in its place.
    ///
    /// This is how watch and server mode pick up code changes: rather than
    /// trying to mutate a live interpreter with `importlib.reload` (which is
    /// brittle once classes/closures/decorator-bound state from the old
    /// definitions are referenced from elsewhere), we drop the whole process
    /// and let it re-import everything on the next `run_test`. The fresh
    /// process replays cached `register_hooks` calls so fixtures keep
    /// working — same path as crash recovery.
    pub async fn restart_workers(&self) {
        self.fanout_ctrl(WorkerCtrl::Restart).await;
    }

    /// Pre-spawn all worker processes in parallel so Python startup
    /// latency is not on the critical path of the first tests.
    pub async fn warm(&self) {
        self.fanout_ctrl(WorkerCtrl::Ping).await;
    }

    pub fn shutdown(self) {
        for _ in 0..self.ctrl_txs.len() {
            let _ = self.work_tx.send_blocking(WorkerMsg::Shutdown);
        }
    }
}

pub use tryke_types::path_to_module;

/// Ensure a worker process is live, spawning one if needed and replaying
/// every cached `register_hooks` call before returning it. Replay guarantees
/// that after a crash-and-respawn, subsequent tests still see their
/// fixtures — without replay, the fresh worker would have empty hook
/// metadata and silently skip `before_each` / `after_each`.
async fn ensure_worker<'a>(
    state: &'a mut WorkerState,
    ctx: &WorkerSpawnCtx<'_>,
) -> Option<&'a mut WorkerProcess> {
    if state.process.is_some() {
        return state.process.as_mut();
    }
    trace!("worker_task: spawning process");
    let mut w = match WorkerProcess::spawn(
        ctx.python_bin,
        ctx.path_refs,
        ctx.root,
        ctx.log_level,
        ctx.limits,
    ) {
        Ok(w) => w,
        Err(e) => {
            debug!("worker_task: spawn failed: {e}");
            return None;
        }
    };
    for (module, params) in &state.hook_cache {
        if let Err(e) = w.register_hooks(params.clone()).await {
            // If hook replay fails the worker is in an inconsistent state
            // (some modules registered, some not). Drop it so the next
            // attempt starts from scratch rather than silently running
            // tests without fixtures.
            debug!("worker_task: replay register_hooks for {module} failed: {e}");
            return None;
        }
    }
    state.process = Some(w);
    state.process.as_mut()
}

/// Execute a single test on the worker. On any RPC error we respawn the
/// worker (replaying cached hooks) for the next test but do NOT retry the
/// failing test — a retry could double-execute side effects if the test
/// partially ran before the crash. The failing test is surfaced as
/// `TestOutcome::Error` with the worker's stderr attached for diagnosis.
async fn run_single_test(
    state: &mut WorkerState,
    ctx: &WorkerSpawnCtx<'_>,
    test: tryke_types::TestItem,
    result_tx: &mpsc::UnboundedSender<TestResult>,
) {
    let Some(w) = ensure_worker(state, ctx).await else {
        let _ = result_tx.send(TestResult {
            test,
            outcome: TestOutcome::Error {
                message: "worker unavailable (spawn or hook replay failed)".into(),
            },
            duration: Duration::ZERO,
            stdout: String::new(),
            stderr: String::new(),
        });
        return;
    };
    match w.run_test(&test).await {
        Ok(result) => {
            trace!("worker_task: test {} done", test.name);
            let _ = result_tx.send(result);
        }
        Err(err) => {
            debug!("worker_task: run_test error for {}: {err}", test.name);
            let stderr_output = w.drain_stderr().await;
            // Drop the dead worker; the next call to `ensure_worker` will
            // spawn a fresh one and replay cached hooks so the remaining
            // tests in this unit keep their fixtures.
            state.process = None;
            let mut msg = format!("worker error: {err}");
            if !stderr_output.is_empty() {
                msg.push_str("\nworker stderr:\n");
                msg.push_str(&stderr_output);
            }
            let _ = result_tx.send(TestResult {
                test,
                outcome: TestOutcome::Error { message: msg },
                duration: Duration::ZERO,
                stdout: String::new(),
                stderr: stderr_output,
            });
        }
    }
}

/// Send `register_hooks` to the worker for each unique module in the work
/// unit, caching the call so any respawn later in the unit can replay it.
async fn register_hooks_for_unit(
    state: &mut WorkerState,
    ctx: &WorkerSpawnCtx<'_>,
    hooks: &[HookItem],
    tests: &[tryke_types::TestItem],
) {
    let mut seen = std::collections::HashSet::new();
    for test in tests {
        if !seen.insert(test.module_path.clone()) {
            continue;
        }
        let module_hooks: Vec<crate::protocol::HookWire> = hooks
            .iter()
            .filter(|h| h.module_path == test.module_path)
            .map(|h| crate::protocol::HookWire {
                name: h.name.clone(),
                per: serde_json::to_value(h.per)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default(),
                groups: h.groups.clone(),
                depends_on: h.depends_on.clone(),
                line_number: h.line_number,
            })
            .collect();

        if module_hooks.is_empty() {
            continue;
        }

        let params = RegisterHooksParams {
            module: test.module_path.clone(),
            hooks: module_hooks,
        };
        // Cache before sending so a respawn that races with this call can
        // still replay the correct hooks.
        state
            .hook_cache
            .insert(test.module_path.clone(), params.clone());

        let Some(w) = ensure_worker(state, ctx).await else {
            continue;
        };
        if let Err(e) = w.register_hooks(params).await {
            debug!("worker_task: register_hooks failed: {e}");
            // Worker is potentially wedged; drop it so the next test
            // forces a respawn-with-replay.
            state.process = None;
        }
    }
}

async fn handle_ctrl(state: &mut WorkerState, ctx: &WorkerSpawnCtx<'_>, ctrl: WorkerCtrl) {
    match ctrl {
        WorkerCtrl::Ping(ack_tx) => {
            trace!("worker_task: ping (pre-warm)");
            let _ = ensure_worker(state, ctx).await;
            let _ = ack_tx.send(());
        }
        WorkerCtrl::Restart(ack_tx) => {
            trace!("worker_task: restart");
            if let Some(mut w) = state.process.take() {
                w.shutdown().await;
            }
            // Eagerly respawn so the next Unit doesn't pay Python startup
            // latency. ensure_worker replays cached register_hooks against
            // the fresh process, mirroring the crash-recovery path.
            let _ = ensure_worker(state, ctx).await;
            let _ = ack_tx.send(());
        }
    }
}

async fn handle_unit(
    state: &mut WorkerState,
    ctx: &WorkerSpawnCtx<'_>,
    unit: WorkUnit,
    result_tx: mpsc::UnboundedSender<TestResult>,
) {
    if !unit.hooks.is_empty() {
        register_hooks_for_unit(state, ctx, &unit.hooks, &unit.tests).await;
    }
    let finalize_modules: std::collections::HashSet<String> = if unit.hooks.is_empty() {
        std::collections::HashSet::new()
    } else {
        unit.tests.iter().map(|t| t.module_path.clone()).collect()
    };
    for test in unit.tests {
        trace!("worker_task: running test {}", test.name);
        run_single_test(state, ctx, test, &result_tx).await;
    }
    for module in finalize_modules {
        if let Some(w) = state.process.as_mut()
            && let Err(e) = w.finalize_hooks(module).await
        {
            debug!("worker_task: finalize_hooks failed: {e}");
        }
    }

    // Recycle a worker that has tripped any of its soft resource caps.
    // Deferred to the end of the unit (after finalize_hooks) so
    // per="scope" fixture teardown is not skipped: recycling mid-unit
    // would drop the live process before its scope fixtures got their
    // teardown call. The next unit handed to this worker_task hits
    // ensure_worker, which spawns a fresh process and replays cached
    // hooks — same path as crash recovery.
    if let Some(reason) = state
        .process
        .as_ref()
        .and_then(WorkerProcess::should_recycle)
        && let Some(mut proc) = state.process.take()
    {
        debug!("worker_task: recycling worker ({reason})");
        proc.shutdown().await;
    }
}

async fn worker_task(
    python_bin: String,
    python_path: Vec<std::path::PathBuf>,
    root: PathBuf,
    log_level: LevelFilter,
    limits: WorkerLimits,
    work_rx: async_channel::Receiver<WorkerMsg>,
    mut ctrl_rx: mpsc::UnboundedReceiver<WorkerCtrl>,
) {
    let path_refs: Vec<&Path> = python_path.iter().map(PathBuf::as_path).collect();
    let ctx = WorkerSpawnCtx {
        python_bin: &python_bin,
        path_refs: &path_refs,
        root: &root,
        log_level,
        limits,
    };
    let mut state = WorkerState::new();

    loop {
        // `biased` guarantees control messages take priority once the
        // current Unit (if any) finishes. Without it, `select!` could
        // keep picking Units off the shared queue while a Restart sat
        // in this worker's ctrl channel — leaving the worker on a stale
        // interpreter for arbitrarily long.
        tokio::select! {
            biased;
            ctrl = ctrl_rx.recv() => {
                let Some(ctrl) = ctrl else { break };
                handle_ctrl(&mut state, &ctx, ctrl).await;
            }
            msg = work_rx.recv() => {
                match msg {
                    Ok(WorkerMsg::Unit(unit, result_tx)) => {
                        handle_unit(&mut state, &ctx, unit, result_tx).await;
                    }
                    Ok(WorkerMsg::Shutdown) | Err(_) => break,
                }
            }
        }
    }

    if let Some(mut w) = state.process.take() {
        w.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tokio_stream::StreamExt;
    use tryke_testing::python_bin as test_python_bin;
    use tryke_types::{FixturePer, HookItem, TestItem};

    use super::*;
    use crate::schedule::WorkUnit;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root")
    }

    fn python_package_dir() -> PathBuf {
        workspace_root().join("python")
    }

    fn make_test_item(module: &str, name: &str, file: &std::path::Path) -> TestItem {
        TestItem {
            name: name.to_string(),
            module_path: module.to_string(),
            file_path: Some(file.to_path_buf()),
            ..TestItem::default()
        }
    }

    /// End-to-end crash-recovery test: a middle test crashes the worker;
    /// the failure must surface as `TestOutcome::Error` for exactly that
    /// test, subsequent tests in the unit must still run with their
    /// fixtures (hooks replayed on respawn), and the crashing test must
    /// NOT be retried (no double-execution of side effects).
    #[tokio::test]
    async fn worker_crash_replays_hooks_and_does_not_double_execute() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");

        let crash_counter = dir.path().join("CRASH_COUNT");
        let crash_counter_escaped = crash_counter.to_string_lossy().replace('\\', "\\\\");
        let test_file = dir.path().join("test_crash.py");
        let source = format!(
            r#"from tryke import test, fixture, Depends, expect

@fixture
def counter() -> int:
    return 42

@test
def test_first(n: int = Depends(counter)) -> None:
    expect(n).to_equal(42)

@test
def test_crasher() -> None:
    import os
    with open("{crash_counter_escaped}", "a") as f:
        f.write("x")
        f.flush()
    os._exit(1)

@test
def test_third(n: int = Depends(counter)) -> None:
    expect(n).to_equal(42)
"#
        );
        std::fs::write(&test_file, source).expect("write test file");

        let hook = HookItem {
            name: "counter".into(),
            module_path: "test_crash".into(),
            per: FixturePer::Test,
            groups: vec![],
            depends_on: vec![],
            line_number: None,
        };
        let tests = vec![
            make_test_item("test_crash", "test_first", &test_file),
            make_test_item("test_crash", "test_crasher", &test_file),
            make_test_item("test_crash", "test_third", &test_file),
        ];
        let unit = WorkUnit {
            tests,
            hooks: vec![hook],
        };

        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_package_dir()],
            LevelFilter::Off,
        );
        pool.warm().await;

        let mut results: Vec<TestResult> = pool.run(vec![unit]).collect().await;
        results.sort_by_key(|r| r.test.name.clone());

        assert_eq!(results.len(), 3, "expected 3 results, got {results:?}");
        // Sorted: test_crasher, test_first, test_third
        let crasher = &results[0];
        let first = &results[1];
        let third = &results[2];

        assert_eq!(crasher.test.name, "test_crasher");
        assert!(
            matches!(crasher.outcome, TestOutcome::Error { .. }),
            "crasher should be Error, got {:?}",
            crasher.outcome
        );
        assert!(
            matches!(first.outcome, TestOutcome::Passed),
            "first should pass (fixture wired), got {:?}",
            first.outcome
        );
        assert!(
            matches!(third.outcome, TestOutcome::Passed),
            "third should pass after respawn+hook-replay, got {:?}",
            third.outcome
        );

        let count = std::fs::read_to_string(&crash_counter).unwrap_or_default();
        assert_eq!(
            count.len(),
            1,
            "crashing test must run exactly once (no retry), got {count:?}"
        );

        pool.shutdown();
    }

    /// Restarting the pool must yield a *fresh* Python interpreter — not
    /// just an `importlib.reload`-mutated module. We prove this by
    /// recording one tally mark per fresh import of the test module: the
    /// module body increments a sidecar counter on every initial load.
    /// Importlib.reload would re-run the body too, but in production it
    /// leaves classes/closures bound to the old definitions in *other*
    /// modules — the brittleness this rearchitecture exists to fix.
    /// A second tally after `restart_workers` confirms a brand new
    /// interpreter is in play.
    #[tokio::test]
    async fn restart_workers_runs_module_body_on_fresh_interpreter() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");

        let counter_file = dir.path().join("IMPORT_COUNT");
        let counter_escaped = counter_file.to_string_lossy().replace('\\', "\\\\");
        let test_file = dir.path().join("test_restart_state.py");
        let source = format!(
            r#"from tryke import test, expect

with open("{counter_escaped}", "a") as f:
    f.write("x")
    f.flush()

@test
def test_noop() -> None:
    expect(1).to_equal(1)
"#
        );
        std::fs::write(&test_file, source).expect("write test file");

        let make_unit = || WorkUnit {
            tests: vec![make_test_item(
                "test_restart_state",
                "test_noop",
                &test_file,
            )],
            hooks: vec![],
        };

        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_package_dir()],
            LevelFilter::Off,
        );
        pool.warm().await;

        let r1: Vec<TestResult> = pool.run(vec![make_unit()]).collect().await;
        assert_eq!(r1.len(), 1);
        assert!(
            matches!(r1[0].outcome, TestOutcome::Passed),
            "first run should pass, got {:?}",
            r1[0].outcome
        );

        pool.restart_workers().await;

        let r2: Vec<TestResult> = pool.run(vec![make_unit()]).collect().await;
        assert_eq!(r2.len(), 1);
        assert!(
            matches!(r2[0].outcome, TestOutcome::Passed),
            "second run should pass on fresh interpreter, got {:?}",
            r2[0].outcome
        );

        let count = std::fs::read_to_string(&counter_file).unwrap_or_default();
        assert_eq!(
            count.len(),
            2,
            "module body must run once per fresh interpreter \
             (1 initial + 1 after restart_workers); got {count:?}"
        );

        pool.shutdown();
    }

    /// `restart_workers` on a pool that has not been warmed (no
    /// processes spawned yet) must still ack. The handler eagerly
    /// spawns a fresh process — same path as a normal restart — so the
    /// next test run is not on the Python startup critical path. This
    /// matters because the file watcher can fire before the user
    /// triggers any test run, and the watcher awaits the ack.
    #[tokio::test]
    async fn restart_workers_with_no_live_processes_acks() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");

        let pool = WorkerPool::with_python_path(
            2,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_package_dir()],
            LevelFilter::Off,
        );
        // No warm() — workers are not yet spawned.
        let restarted =
            tokio::time::timeout(std::time::Duration::from_secs(10), pool.restart_workers()).await;
        assert!(restarted.is_ok(), "restart_workers must ack within timeout");

        pool.shutdown();
    }

    /// A worker process must be recycled when its self-reported
    /// resource snapshot crosses a configured ceiling so accumulated
    /// module-level state (and the FDs it owns) does not grow
    /// unboundedly across long runs. We use the wall-clock `max_age`
    /// signal here because it is the easiest to drive deterministically
    /// from a test (memory and FD growth would require platform-
    /// specific allocations, and the priority logic is unit-tested
    /// separately in `worker.rs`).
    ///
    /// The recycle is deferred until the end of a unit (so per="scope"
    /// teardown is not skipped — see
    /// `recycle_does_not_skip_scope_fixture_teardown`), so we exercise
    /// it here by sleeping past the cap between units. The module body
    /// records the worker pid on every fresh import; observing ≥ 2
    /// distinct pids proves the age recycle fired at least once.
    ///
    /// We deliberately do NOT pin the count to exactly 2: on slow CI
    /// runners (cold Python startup, busy schedulers) the first unit's
    /// end-of-unit check can already cross a tight cap, producing a
    /// recycle before the sleep. That's not a correctness regression —
    /// the property under test ("age cap eventually fires") still
    /// holds. The complementary "no recycle when under cap" property
    /// is covered by `evaluate_recycle_returns_none_when_no_signals_tripped`.
    #[tokio::test]
    async fn worker_recycles_when_age_exceeds_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");

        let pid_log = dir.path().join("PID_LOG");
        let pid_log_escaped = pid_log.to_string_lossy().replace('\\', "\\\\");
        let test_file = dir.path().join("test_recycle.py");
        let source = format!(
            r#"import os

# Module body runs once per fresh interpreter — record this worker's pid so
# the test can count distinct workers (one per recycle).
with open("{pid_log_escaped}", "a") as f:
    f.write(str(os.getpid()) + "\n")
    f.flush()

from tryke import test, expect

@test
def test_noop() -> None:
    expect(1).to_equal(1)
"#
        );
        std::fs::write(&test_file, source).expect("write test file");

        // Tight age cap — a sleep between units crosses it
        // deterministically without making the test slow. Slow CI
        // runners may also cross it within a single unit; see the
        // assertion at the bottom for why that's acceptable.
        let limits = WorkerLimits {
            max_rss_bytes: None,
            max_open_fds: None,
            max_age: Some(std::time::Duration::from_millis(100)),
        };
        let pool = WorkerPool::with_python_path_and_limits(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_package_dir()],
            LevelFilter::Off,
            limits,
        );
        pool.warm().await;

        let make_unit = || WorkUnit {
            tests: vec![make_test_item("test_recycle", "test_noop", &test_file)],
            hooks: vec![],
        };

        let r1: Vec<TestResult> = pool.run(vec![make_unit()]).collect().await;
        assert_eq!(r1.len(), 1);
        assert!(matches!(r1[0].outcome, TestOutcome::Passed));

        // Sleep past the age cap so the next unit's end-of-unit check
        // fires. 250ms gives comfortable margin over the 100ms cap on
        // jittery CI without pushing test latency higher than necessary.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;

        let r2: Vec<TestResult> = pool.run(vec![make_unit()]).collect().await;
        assert_eq!(r2.len(), 1);
        assert!(matches!(r2[0].outcome, TestOutcome::Passed));

        // Third unit forces a fresh spawn (the prior worker was
        // recycled at the end of unit 2 at latest), guaranteeing the
        // pid log gains a new entry.
        let r3: Vec<TestResult> = pool.run(vec![make_unit()]).collect().await;
        assert_eq!(r3.len(), 1);
        assert!(matches!(r3[0].outcome, TestOutcome::Passed));

        let pid_lines = std::fs::read_to_string(&pid_log).unwrap_or_default();
        let distinct_pids: std::collections::HashSet<&str> =
            pid_lines.lines().filter(|l| !l.is_empty()).collect();
        assert!(
            distinct_pids.len() >= 2,
            "expected ≥ 2 distinct worker pid(s) (recycle fired at least once); \
             got {} from log: {pid_lines:?}",
            distinct_pids.len(),
        );

        pool.shutdown();
    }

    /// Recycling must never strand a `per="scope"` fixture without its
    /// teardown. The unit body's tests must still see their fixture
    /// values, and the scope fixture's `yield`-after teardown must run
    /// exactly once before the worker is recycled. A buggy version
    /// that recycled mid-unit (before `finalize_hooks`) would either
    /// strand teardown entirely or — if the fresh worker re-ran setup
    /// — record setup twice with teardown still at one or zero.
    ///
    /// The recycle is forced via a tiny `max_age` so a short sleep
    /// inside the unit's first test crosses the cap; the end-of-unit
    /// check then trips as soon as the unit completes (after
    /// finalize). We assert setup and teardown both ran exactly once.
    #[tokio::test]
    async fn recycle_does_not_skip_scope_fixture_teardown() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");

        let setup_log = dir.path().join("SETUP_LOG");
        let teardown_log = dir.path().join("TEARDOWN_LOG");
        let setup_escaped = setup_log.to_string_lossy().replace('\\', "\\\\");
        let teardown_escaped = teardown_log.to_string_lossy().replace('\\', "\\\\");
        let test_file = dir.path().join("test_scope_recycle.py");
        let source = format!(
            r#"import os
import time
from tryke import test, fixture, expect, Depends

@fixture(per="scope")
def scope_resource() -> int:
    with open("{setup_escaped}", "a") as f:
        f.write(str(os.getpid()) + "\n")
        f.flush()
    yield 42
    with open("{teardown_escaped}", "a") as f:
        f.write(str(os.getpid()) + "\n")
        f.flush()

@test
def test_sleep_then_age_out(r: int = Depends(scope_resource)) -> None:
    # Sleep past the runner's tiny max_age so the end-of-unit recycle
    # check trips. Teardown must still have a chance to run before the
    # worker is killed.
    time.sleep(0.25)
    expect(r).to_equal(42)

@test
def test_uses_scope(r: int = Depends(scope_resource)) -> None:
    expect(r).to_equal(42)
"#
        );
        std::fs::write(&test_file, source).expect("write test file");

        let hook = HookItem {
            name: "scope_resource".into(),
            module_path: "test_scope_recycle".into(),
            per: FixturePer::Scope,
            groups: vec![],
            depends_on: vec![],
            line_number: None,
        };
        // Two tests in one unit: first sleeps past max_age, second
        // proves teardown wasn't stranded mid-unit (it would have been
        // had recycle fired before finalize_hooks).
        let tests: Vec<TestItem> = vec![
            make_test_item("test_scope_recycle", "test_sleep_then_age_out", &test_file),
            make_test_item("test_scope_recycle", "test_uses_scope", &test_file),
        ];
        let unit = WorkUnit {
            tests,
            hooks: vec![hook],
        };

        let limits = WorkerLimits {
            max_rss_bytes: None,
            max_open_fds: None,
            max_age: Some(std::time::Duration::from_millis(100)),
        };
        let pool = WorkerPool::with_python_path_and_limits(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_package_dir()],
            LevelFilter::Off,
            limits,
        );
        pool.warm().await;

        let results: Vec<TestResult> = pool.run(vec![unit]).collect().await;
        assert_eq!(results.len(), 2, "expected 2 results, got {results:?}");
        for r in &results {
            assert!(
                matches!(r.outcome, TestOutcome::Passed),
                "test failed unexpectedly: {r:?}",
            );
        }

        let setup_lines = std::fs::read_to_string(&setup_log).unwrap_or_default();
        let teardown_lines = std::fs::read_to_string(&teardown_log).unwrap_or_default();
        let setup_count = setup_lines.lines().filter(|l| !l.is_empty()).count();
        let teardown_count = teardown_lines.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(
            setup_count, 1,
            "scope fixture setup must run exactly once for the unit; \
             got {setup_count} from log: {setup_lines:?}",
        );
        assert_eq!(
            teardown_count, 1,
            "scope fixture teardown must run exactly once for the unit \
             (recycling must not strand teardown); got {teardown_count} \
             from log: {teardown_lines:?}",
        );

        pool.shutdown();
    }
}
