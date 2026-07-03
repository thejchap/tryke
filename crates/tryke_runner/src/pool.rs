use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use log::{LevelFilter, debug, trace, warn};

use tokio::sync::{mpsc, oneshot};
use tokio_stream::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tryke_types::{HookItem, TestOutcome, TestResult};

use crate::protocol::RegisterHooksParams;
use crate::schedule::WorkUnit;
use crate::worker::WorkerProcess;

const WORKER_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const WORKER_SPAWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-worker-task state: the (optional) live Python process plus a cache of
/// the most recent `register_hooks` call per module. The cache exists so
/// that a freshly-spawned worker (after a crash) can be brought back to the
/// same hook-registration state as the one it replaces — otherwise
/// subsequent tests in the same unit would silently run without their
/// `before_each` / `after_each` fixtures.
struct WorkerState {
    process: Option<WorkerProcess>,
    hook_cache: HashMap<String, RegisterHooksParams>,
    /// Most recent spawn or hook-replay failure, captured so
    /// `run_single_test` can surface the real reason (and any worker
    /// stderr) instead of the opaque "worker unavailable" placeholder.
    /// Cleared once we have a live worker again so we don't replay a
    /// stale error against an unrelated test.
    last_failure: Option<String>,
}

impl WorkerState {
    fn new() -> Self {
        Self {
            process: None,
            hook_cache: HashMap::new(),
            last_failure: None,
        }
    }
}

/// Build the user-facing message for a worker error, appending captured
/// stderr (if any) so the python-side traceback is visible to the user
/// without needing to enable debug logging. The actual python crash —
/// e.g. `ModuleNotFoundError: No module named 'tryke'` when the worker
/// venv is missing the package — only ever appears on the worker's
/// stderr pipe, so suppressing it here is what makes spawn failures look
/// like an opaque "worker unavailable".
fn format_worker_failure(prefix: &str, err: &dyn std::fmt::Display, stderr: &str) -> String {
    let mut msg = format!("{prefix}: {err}");
    let trimmed = stderr.trim();
    if !trimmed.is_empty() {
        msg.push_str("\nworker stderr:\n");
        msg.push_str(trimmed);
    }
    msg
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
    /// Sender channel for work units
    ///
    /// Workers claim individual units off of this
    work_tx: async_channel::Sender<WorkerMsg>,

    /// Control channels
    ///
    /// One per-worker to distribute messages to all.
    ctrl_txs: Vec<mpsc::UnboundedSender<WorkerCtrl>>,
}

impl WorkerPool {
    /// Spawns a pool whose workers receive `TRYKE_LOG=<log_level>`.
    ///
    /// Pass `LevelFilter::Off` to leave workers silent (the env var is
    /// then not set on the child, so the worker's
    /// `_configure_logging_from_env` no-ops). Production callers use
    /// `tryke_config::worker_log_level` to derive this from CLI flags
    /// + the `TRYKE_LOG` env var.
    ///
    /// `python_path` overrides the default path of `root` plus its `python`
    /// directory when present. If `warm` is true, this method also waits for
    /// every Python subprocess to start before returning.
    pub async fn spawn(
        size: usize,
        python_bin: &str,
        root: &Path,
        python_path: Option<&[PathBuf]>,
        log_level: LevelFilter,
        warm: bool,
    ) -> Self {
        let size = size.max(1);
        let python_path = python_path.map_or_else(
            || {
                let mut paths = vec![root.to_path_buf()];
                // pyproject.toml declares python-source = "python" — add it to
                // PYTHONPATH so the tryke package is importable without a venv.
                let src_dir = root.join("python");
                if src_dir.is_dir() {
                    paths.push(src_dir);
                }
                paths
            },
            <[PathBuf]>::to_vec,
        );
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
                work_rx,
                ctrl_rx,
            ));
        }

        let pool = Self { work_tx, ctrl_txs };

        if warm {
            pool.warm().await;
        }

        pool
    }

    /// Submit any number of `WorkUnit`s to the worker pool
    ///
    /// A `WorkUnit` is an atomic group of tests to be run sequentially on a single worker
    /// Returns a stream
    pub fn submit(&self, units: Vec<WorkUnit>) -> impl Stream<Item = TestResult> + use<> {
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
    async fn fanout_ctrl_with_timeout(
        &self,
        build: fn(oneshot::Sender<()>) -> WorkerCtrl,
        timeout: Duration,
    ) -> bool {
        let mut ack_rxs = Vec::with_capacity(self.ctrl_txs.len());
        for ctrl_tx in &self.ctrl_txs {
            let (ack_tx, ack_rx) = oneshot::channel();
            if ctrl_tx.send(build(ack_tx)).is_ok() {
                ack_rxs.push(ack_rx);
            }
        }
        tokio::time::timeout(timeout, async {
            for ack_rx in ack_rxs {
                let _ = ack_rx.await;
            }
        })
        .await
        .is_ok()
    }

    async fn fanout_ctrl(&self, operation: &str, build: fn(oneshot::Sender<()>) -> WorkerCtrl) {
        if !self
            .fanout_ctrl_with_timeout(build, WORKER_CONTROL_TIMEOUT)
            .await
        {
            // A control timeout means at least one worker did not ack a
            // restart/warm, undermining the "fresh workers each run"
            // guarantee — surface it at warn so it's visible by default.
            warn!(
                "worker control operation '{operation}' timed out after {WORKER_CONTROL_TIMEOUT:?}"
            );
        }
    }

    /// Replace every worker subprocess with a fresh, responsive process.
    ///
    /// This is how watch and server mode pick up code changes: rather than
    /// trying to mutate a live interpreter with `importlib.reload` (which is
    /// brittle once classes/closures/decorator-bound state from the old
    /// definitions are referenced from elsewhere), we drop the whole process
    /// and let it re-import everything on the next `run_test`. The fresh
    /// process replays cached `register_hooks` calls so fixtures keep
    /// working — same path as crash recovery.
    pub async fn restart_workers(&self) {
        self.fanout_ctrl("restart", WorkerCtrl::Restart).await;
        self.warm().await;
    }

    /// Pre-spawn all worker processes in parallel so Python startup
    /// latency is not on the critical path of the first tests.
    async fn warm(&self) {
        self.fanout_ctrl("warm", WorkerCtrl::Ping).await;
    }

    pub fn shutdown(self) {
        for _ in 0..self.ctrl_txs.len() {
            let _ = self.work_tx.send_blocking(WorkerMsg::Shutdown);
        }
    }
}

pub use tryke_types::path_to_module;

async fn spawn_worker_process(
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
    log_level: LevelFilter,
) -> Result<WorkerProcess> {
    let python_bin = python_bin.to_owned();
    let python_paths = path_refs
        .iter()
        .map(|path| (*path).to_path_buf())
        .collect::<Vec<_>>();
    let root = root.to_path_buf();
    let spawn = tokio::task::spawn_blocking(move || {
        let path_refs = python_paths
            .iter()
            .map(PathBuf::as_path)
            .collect::<Vec<_>>();
        WorkerProcess::spawn(&python_bin, &path_refs, &root, log_level)
    });

    match tokio::time::timeout(WORKER_SPAWN_TIMEOUT, spawn).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(anyhow!("worker spawn task failed: {error}")),
        Err(_) => Err(anyhow!(
            "worker process spawn timed out after {WORKER_SPAWN_TIMEOUT:?}"
        )),
    }
}

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
    log_level: LevelFilter,
) -> Option<&'a mut WorkerProcess> {
    if state.process.is_some() {
        return state.process.as_mut();
    }
    trace!("worker_task: spawning process");
    let mut w = match spawn_worker_process(python_bin, path_refs, root, log_level).await {
        Ok(w) => w,
        Err(e) => {
            let msg = format_worker_failure(
                &format!("failed to spawn python worker ({python_bin} -m tryke.worker)"),
                &e,
                "",
            );
            debug!("worker_task: {msg}");
            state.last_failure = Some(msg);
            return None;
        }
    };
    for (module, params) in &state.hook_cache {
        if let Err(e) = w.register_hooks(params.clone()).await {
            // Drain stderr before dropping the dead worker — otherwise
            // the python traceback that explains *why* replay failed
            // (e.g. ModuleNotFoundError on the worker side) goes with
            // it and the user sees only "Broken pipe".
            let stderr_output = w.drain_stderr().await;
            let msg = format_worker_failure(
                &format!("hook replay failed for module {module}"),
                &e,
                &stderr_output,
            );
            debug!("worker_task: {msg}");
            state.last_failure = Some(msg);
            // Worker is in an inconsistent state (some modules registered,
            // some not). Drop it so the next attempt starts from scratch
            // rather than silently running tests without fixtures.
            return None;
        }
    }
    state.last_failure = None;
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
    log_level: LevelFilter,
    test: tryke_types::TestItem,
    result_tx: &mpsc::UnboundedSender<TestResult>,
) {
    let Some(w) = ensure_worker(state, python_bin, path_refs, root, log_level).await else {
        let message = state
            .last_failure
            .clone()
            .unwrap_or_else(|| "worker unavailable (spawn or hook replay failed)".into());
        let _ = result_tx.send(TestResult {
            test,
            outcome: TestOutcome::Error { message },
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
            let message = format_worker_failure("worker error", &err, &stderr_output);
            let _ = result_tx.send(TestResult {
                test,
                outcome: TestOutcome::Error { message },
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
    log_level: LevelFilter,
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

        let Some(w) = ensure_worker(state, python_bin, path_refs, root, log_level).await else {
            continue;
        };
        if let Err(e) = w.register_hooks(params).await {
            // Drain stderr before dropping so the python traceback that
            // killed the worker reaches the user-facing test result via
            // `last_failure`, instead of being lost with the process.
            let stderr_output = w.drain_stderr().await;
            let msg = format_worker_failure(
                &format!("register_hooks failed for module {}", test.module_path),
                &e,
                &stderr_output,
            );
            debug!("worker_task: {msg}");
            state.last_failure = Some(msg);
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
    log_level: LevelFilter,
    ctrl: WorkerCtrl,
) {
    match ctrl {
        WorkerCtrl::Ping(ack_tx) => {
            trace!("worker_task: ping (pre-warm)");
            let _ = ensure_worker(state, python_bin, path_refs, root, log_level).await;
            let _ = ack_tx.send(());
        }
        WorkerCtrl::Restart(ack_tx) => {
            trace!("worker_task: restart");
            if let Some(mut w) = state.process.take() {
                w.shutdown().await;
            }
            let _ = ack_tx.send(());
        }
    }
}

async fn handle_unit(
    state: &mut WorkerState,
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
    log_level: LevelFilter,
    unit: WorkUnit,
    result_tx: mpsc::UnboundedSender<TestResult>,
) {
    if !unit.hooks.is_empty() {
        register_hooks_for_unit(
            state,
            python_bin,
            path_refs,
            root,
            log_level,
            &unit.hooks,
            &unit.tests,
        )
        .await;
    }
    let finalize_modules: std::collections::HashSet<String> =
        unit.tests.iter().map(|t| t.module_path.clone()).collect();
    for test in unit.tests {
        trace!("worker_task: running test {}", test.name);
        run_single_test(
            state, python_bin, path_refs, root, log_level, test, &result_tx,
        )
        .await;
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
    log_level: LevelFilter,
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
                handle_ctrl(&mut state, &python_bin, &path_refs, &root, log_level, ctrl).await;
            }
            msg = work_rx.recv() => {
                match msg {
                    Ok(WorkerMsg::Unit(unit, result_tx)) => {
                        handle_unit(
                            &mut state,
                            &python_bin,
                            &path_refs,
                            &root,
                            log_level,
                            unit,
                            result_tx,
                        )
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

        let python_path = [dir.path().to_path_buf(), python_package_dir()];
        let pool = WorkerPool::spawn(
            1,
            &test_python_bin(),
            dir.path(),
            Some(&python_path),
            LevelFilter::Off,
            true,
        )
        .await;

        let mut results: Vec<TestResult> = pool.submit(vec![unit]).collect().await;
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

        let python_path = [dir.path().to_path_buf(), python_package_dir()];
        let pool = WorkerPool::spawn(
            1,
            &test_python_bin(),
            dir.path(),
            Some(&python_path),
            LevelFilter::Off,
            true,
        )
        .await;

        let r1: Vec<TestResult> = pool.submit(vec![make_unit()]).collect().await;
        assert_eq!(r1.len(), 1);
        assert!(
            matches!(r1[0].outcome, TestOutcome::Passed),
            "first run should pass, got {:?}",
            r1[0].outcome
        );

        pool.restart_workers().await;

        let r2: Vec<TestResult> = pool.submit(vec![make_unit()]).collect().await;
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

    /// When the worker python dies during startup (e.g. project venv
    /// without `tryke` installed prints `ModuleNotFoundError` and
    /// exits), the user-facing error must include the python stderr —
    /// not just the opaque "worker unavailable" placeholder that used
    /// to be all the user saw. Regression test for the diagnosability
    /// fix.
    ///
    /// The unit carries a `HookItem` on purpose: with `hooks: vec![]`,
    /// `handle_unit` skips `register_hooks_for_unit` entirely and the
    /// failure would surface via `run_single_test`'s `run_test` error
    /// path — which already existed before this PR. To exercise the
    /// new code (`register_hooks_for_unit` stashing `last_failure`
    /// after `drain_stderr`, then `ensure_worker`'s replay loop
    /// stashing again on respawn, then `run_single_test` reading
    /// `last_failure` instead of the opaque placeholder) the test
    /// needs a hook so `register_hooks_for_unit` actually runs.
    ///
    /// Unix-only: simulates the failing python with a shell script.
    /// The workspace venv's python has `tryke` installed in editable
    /// mode and its `.pth` file is searched regardless of PYTHONPATH,
    /// so we can't reproduce the missing-module case with a real
    /// interpreter. A stub `python_bin` is sufficient — what we're
    /// testing is the rust-side error propagation, not python's
    /// resolution rules.
    #[cfg(unix)]
    #[tokio::test]
    async fn worker_missing_tryke_surfaces_python_error_in_outcome() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");

        let fake_python = dir.path().join("fake_python.sh");
        std::fs::write(
            &fake_python,
            "#!/bin/sh\n\
             echo \"$0: Error while finding module specification for \
             'tryke.worker' (ModuleNotFoundError: No module named 'tryke')\" >&2\n\
             exit 1\n",
        )
        .expect("write fake python");
        std::fs::set_permissions(&fake_python, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake python");

        let test_file = dir.path().join("test_no_tryke.py");
        std::fs::write(&test_file, "def test_noop(): pass\n").expect("write test file");

        // Hook on the same module forces `handle_unit` through
        // `register_hooks_for_unit` → `ensure_worker` (spawn ok) →
        // `register_hooks` (RPC error) → `drain_stderr` → stash
        // `last_failure` → drop process. Then `run_single_test` →
        // `ensure_worker` → respawn ok → replay `register_hooks`
        // (RPC error) → stash `last_failure` → return None →
        // `run_single_test` surfaces `last_failure` as the outcome
        // message. Without this hook the new path isn't exercised.
        let hook = HookItem {
            name: "noop".into(),
            module_path: "test_no_tryke".into(),
            per: FixturePer::Test,
            groups: vec![],
            depends_on: vec![],
            line_number: None,
        };
        let unit = WorkUnit {
            tests: vec![make_test_item("test_no_tryke", "test_noop", &test_file)],
            hooks: vec![hook],
        };

        let python_path = [dir.path().to_path_buf()];
        let pool = WorkerPool::spawn(
            1,
            fake_python.to_str().expect("fake python path"),
            dir.path(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;

        let results: Vec<TestResult> = pool.submit(vec![unit]).collect().await;
        assert_eq!(results.len(), 1);
        let message = match &results[0].outcome {
            TestOutcome::Error { message } => message.clone(),
            other => panic!("expected Error outcome, got {other:?}"),
        };
        // `run_single_test`'s `last_failure` path produces this prefix
        // — proves we went through `ensure_worker`'s replay loop, not
        // through `run_test`'s direct error path which would say
        // "worker error: …".
        assert!(
            message.starts_with("hook replay failed for module test_no_tryke"),
            "expected hook-replay prefix from ensure_worker.last_failure, got: {message}"
        );
        assert!(
            message.contains("No module named 'tryke'"),
            "missing python traceback in error message: {message}"
        );
        assert!(
            message.contains("worker stderr:"),
            "missing 'worker stderr:' header in error message: {message}"
        );

        pool.shutdown();
    }

    /// `restart_workers` on a cold pool must start every process and
    /// acknowledge within the control timeout. This matters because the file
    /// watcher can fire before the user triggers any test run.
    #[tokio::test]
    async fn restart_workers_with_no_live_processes_acks() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");

        let python_path = [dir.path().to_path_buf(), python_package_dir()];
        let pool = WorkerPool::spawn(
            2,
            &test_python_bin(),
            dir.path(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;
        let restarted =
            tokio::time::timeout(std::time::Duration::from_secs(10), pool.restart_workers()).await;
        assert!(restarted.is_ok(), "restart_workers must ack within timeout");

        pool.shutdown();
    }

    #[tokio::test]
    async fn worker_control_fanout_times_out_when_a_worker_does_not_ack() {
        let (work_tx, _work_rx) = async_channel::unbounded();
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let pool = WorkerPool {
            work_tx,
            ctrl_txs: vec![ctrl_tx],
        };

        let acknowledged = pool
            .fanout_ctrl_with_timeout(WorkerCtrl::Restart, Duration::from_millis(10))
            .await;

        assert!(!acknowledged, "a missing worker ack must time out");
    }
}
