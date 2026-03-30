use std::path::{Path, PathBuf};
use std::time::Duration;

use log::{debug, trace};

use tokio::sync::{mpsc, oneshot};
use tokio_stream::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tryke_types::{HookItem, TestOutcome, TestResult};

use crate::schedule::WorkUnit;
use crate::worker::WorkerProcess;

enum WorkerMsg {
    Ping(oneshot::Sender<()>),
    Unit(WorkUnit, mpsc::UnboundedSender<TestResult>),
    Reload(Vec<String>, oneshot::Sender<()>),
    Shutdown,
}

pub struct WorkerPool {
    work_tx: async_channel::Sender<WorkerMsg>,
    size: usize,
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
        for _ in 0..size {
            let bin = python_bin.to_owned();
            let rx = work_rx.clone();
            tokio::spawn(worker_task(bin, python_path.clone(), root.clone(), rx));
        }
        Self { work_tx, size }
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

    pub async fn reload(&self, modules: Vec<String>) {
        // Reload must reach every worker, so we send N messages (one per worker)
        // and wait for all acknowledgements.
        let mut ack_rxs = Vec::with_capacity(self.size);
        for _ in 0..self.size {
            let (ack_tx, ack_rx) = oneshot::channel();
            let _ = self
                .work_tx
                .send(WorkerMsg::Reload(modules.clone(), ack_tx))
                .await;
            ack_rxs.push(ack_rx);
        }
        for ack_rx in ack_rxs {
            let _ = ack_rx.await;
        }
    }

    /// Pre-spawn all worker processes in parallel so Python startup
    /// latency is not on the critical path of the first tests.
    pub async fn warm(&self) {
        // Send one Ping per worker so each worker spawns its process.
        let mut ack_rxs = Vec::with_capacity(self.size);
        for _ in 0..self.size {
            let (ack_tx, ack_rx) = oneshot::channel();
            let _ = self.work_tx.send(WorkerMsg::Ping(ack_tx)).await;
            ack_rxs.push(ack_rx);
        }
        for ack_rx in ack_rxs {
            let _ = ack_rx.await;
        }
    }

    pub fn shutdown(self) {
        for _ in 0..self.size {
            let _ = self.work_tx.send_blocking(WorkerMsg::Shutdown);
        }
    }
}

pub use tryke_types::path_to_module;

/// Ensure a worker process is spawned, returning a mutable reference.
/// On spawn failure, sends an error result for `test` and returns `None`.
fn ensure_worker<'a>(
    worker: &'a mut Option<WorkerProcess>,
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
    test: &tryke_types::TestItem,
    result_tx: &mpsc::UnboundedSender<TestResult>,
) -> Option<&'a mut WorkerProcess> {
    if worker.is_none() {
        trace!("worker_task: spawning process");
        match WorkerProcess::spawn(python_bin, path_refs, root) {
            Ok(w) => *worker = Some(w),
            Err(e) => {
                let msg = format!("worker spawn failed: {e}");
                debug!("worker_task: {msg}");
                let _ = result_tx.send(TestResult {
                    test: test.clone(),
                    outcome: TestOutcome::Error { message: msg },
                    duration: Duration::ZERO,
                    stdout: String::new(),
                    stderr: String::new(),
                });
                return None;
            }
        }
    }
    worker.as_mut()
}

/// Execute a single test on the worker, retrying once on failure.
async fn run_single_test(
    worker: &mut Option<WorkerProcess>,
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
    test: tryke_types::TestItem,
    result_tx: &mpsc::UnboundedSender<TestResult>,
) {
    let Some(w) = ensure_worker(worker, python_bin, path_refs, root, &test, result_tx) else {
        return;
    };
    match w.run_test(&test).await {
        Ok(result) => {
            trace!("worker_task: test {} done", test.name);
            let _ = result_tx.send(result);
        }
        Err(first_err) => {
            debug!("worker_task: run_test error, respawning for retry");
            let stderr_output = w.drain_stderr().await;
            *worker = WorkerProcess::spawn(python_bin, path_refs, root).ok();
            if let Some(w) = worker.as_mut()
                && let Ok(result) = w.run_test(&test).await
            {
                debug!("worker_task: retry succeeded for {}", test.name);
                let _ = result_tx.send(result);
            } else {
                let mut msg = format!("worker error: {first_err}");
                if !stderr_output.is_empty() {
                    msg.push_str("\nworker stderr:\n");
                    msg.push_str(&stderr_output);
                }
                debug!("worker_task: retry failed for {}", test.name);
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
}

/// Send `register_hooks` to the worker for each unique module in the work unit.
async fn register_hooks_for_unit(
    worker: &mut Option<WorkerProcess>,
    python_bin: &str,
    path_refs: &[&Path],
    root: &Path,
    hooks: &[HookItem],
    tests: &[tryke_types::TestItem],
) {
    // Determine unique modules from the tests.
    let mut seen = std::collections::HashSet::new();
    for test in tests {
        if seen.insert(test.module_path.clone()) {
            // Send all hooks in the unit for this module.
            // Hooks are already scoped to the work unit's file/group.
            let module_hooks: Vec<crate::protocol::HookWire> = hooks
                .iter()
                .map(|h| crate::protocol::HookWire {
                    name: h.name.clone(),
                    hook_type: serde_json::to_value(h.hook_type)
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

            // Ensure worker is spawned.
            if worker.is_none() {
                if let Ok(w) = WorkerProcess::spawn(python_bin, path_refs, root) {
                    *worker = Some(w);
                } else {
                    continue;
                }
            }

            if let Some(w) = worker.as_mut() {
                let params = crate::protocol::RegisterHooksParams {
                    module: test.module_path.clone(),
                    hooks: module_hooks,
                };
                if let Err(e) = w.register_hooks(params).await {
                    debug!("worker_task: register_hooks failed: {e}");
                }
            }
        }
    }
}

async fn worker_task(
    python_bin: String,
    python_path: Vec<std::path::PathBuf>,
    root: PathBuf,
    rx: async_channel::Receiver<WorkerMsg>,
) {
    let path_refs: Vec<&Path> = python_path.iter().map(PathBuf::as_path).collect();
    let mut worker: Option<WorkerProcess> = None;

    while let Ok(msg) = rx.recv().await {
        match msg {
            WorkerMsg::Ping(ack_tx) => {
                trace!("worker_task: ping (pre-warm)");
                if worker.is_none() {
                    trace!("worker_task: spawning process for warm-up");
                    match WorkerProcess::spawn(&python_bin, &path_refs, &root) {
                        Ok(w) => worker = Some(w),
                        Err(e) => {
                            debug!("worker_task: warm-up spawn failed: {e}");
                        }
                    }
                }
                let _ = ack_tx.send(());
            }
            WorkerMsg::Unit(unit, result_tx) => {
                // Send hook metadata for each unique module in this unit.
                if !unit.hooks.is_empty() {
                    register_hooks_for_unit(
                        &mut worker,
                        &python_bin,
                        &path_refs,
                        &root,
                        &unit.hooks,
                        &unit.tests,
                    )
                    .await;
                }
                for test in unit.tests {
                    trace!("worker_task: running test {}", test.name);
                    run_single_test(
                        &mut worker,
                        &python_bin,
                        &path_refs,
                        &root,
                        test,
                        &result_tx,
                    )
                    .await;
                }
            }
            WorkerMsg::Reload(modules, ack_tx) => {
                trace!("worker_task: reload {modules:?}");
                if let Some(w) = worker.as_mut() {
                    let _ = w.reload(&modules).await;
                }
                let _ = ack_tx.send(());
            }
            WorkerMsg::Shutdown => {
                trace!("worker_task: shutdown");
                break;
            }
        }
    }

    if let Some(mut w) = worker {
        w.shutdown().await;
    }
}
