use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use log::{debug, trace};

use tokio::sync::{mpsc, oneshot};
use tokio_stream::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tryke_types::{HookItem, TestOutcome, TestResult};

use crate::protocol::RegisterHooksParams;
use crate::schedule::WorkUnit;
use crate::worker::WorkerProcess;

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
    #[must_use]
    pub fn new(size: usize, python_bin: &str, root: &Path) -> Self {
        let mut python_path = vec![root.to_path_buf()];
        // pyproject.toml declares python-source = "python" — add it to
        // PYTHONPATH so the tryke package is importable even without a venv.
        let src_dir = root.join("python");
        if src_dir.is_dir() {
            python_path.push(src_dir);
        }
        Self::with_python_path(size, python_bin, root, &python_path)
    }

    #[must_use]
    pub fn with_python_path(
        size: usize,
        python_bin: &str,
        root: &Path,
        python_path: &[PathBuf],
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
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
) -> Option<&'a mut WorkerProcess> {
    if state.process.is_some() {
        return state.process.as_mut();
    }
    trace!("worker_task: spawning process");
    let mut w = match WorkerProcess::spawn(python_bin, path_refs, root) {
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
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
    test: tryke_types::TestItem,
    result_tx: &mpsc::UnboundedSender<TestResult>,
) {
    let Some(w) = ensure_worker(state, python_bin, path_refs, root).await else {
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
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
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

        let Some(w) = ensure_worker(state, python_bin, path_refs, root).await else {
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

async fn handle_ctrl(
    state: &mut WorkerState,
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
    ctrl: WorkerCtrl,
) {
    match ctrl {
        WorkerCtrl::Ping(ack_tx) => {
            trace!("worker_task: ping (pre-warm)");
            let _ = ensure_worker(state, python_bin, path_refs, root).await;
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
            let _ = ensure_worker(state, python_bin, path_refs, root).await;
            let _ = ack_tx.send(());
        }
    }
}

async fn handle_unit(
    state: &mut WorkerState,
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
    unit: WorkUnit,
    result_tx: mpsc::UnboundedSender<TestResult>,
) {
    if !unit.hooks.is_empty() {
        register_hooks_for_unit(state, python_bin, path_refs, root, &unit.hooks, &unit.tests).await;
    }
    let finalize_modules: std::collections::HashSet<String> = if unit.hooks.is_empty() {
        std::collections::HashSet::new()
    } else {
        unit.tests.iter().map(|t| t.module_path.clone()).collect()
    };
    for test in unit.tests {
        trace!("worker_task: running test {}", test.name);
        run_single_test(state, python_bin, path_refs, root, test, &result_tx).await;
    }
    for module in finalize_modules {
        if let Some(w) = state.process.as_mut()
            && let Err(e) = w.finalize_hooks(module).await
        {
            debug!("worker_task: finalize_hooks failed: {e}");
        }
    }
}

async fn worker_task(
    python_bin: String,
    python_path: Vec<std::path::PathBuf>,
    root: PathBuf,
    work_rx: async_channel::Receiver<WorkerMsg>,
    mut ctrl_rx: mpsc::UnboundedReceiver<WorkerCtrl>,
) {
    let path_refs: Vec<&Path> = python_path.iter().map(PathBuf::as_path).collect();
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
                handle_ctrl(&mut state, &python_bin, &path_refs, &root, ctrl).await;
            }
            msg = work_rx.recv() => {
                match msg {
                    Ok(WorkerMsg::Unit(unit, result_tx)) => {
                        handle_unit(&mut state, &python_bin, &path_refs, &root, unit, result_tx)
                            .await;
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
    use tryke_test_support::python_bin as test_python_bin;
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
        );
        // No warm() — workers are not yet spawned.
        let restarted =
            tokio::time::timeout(std::time::Duration::from_secs(10), pool.restart_workers()).await;
        assert!(restarted.is_ok(), "restart_workers must ack within timeout");

        pool.shutdown();
    }
}
