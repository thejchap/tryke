use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use log::debug;

use tokio::sync::{mpsc, oneshot};
use tokio_stream::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tryke_types::{TestItem, TestOutcome, TestResult};

use crate::worker::WorkerProcess;

enum WorkerMsg {
    Test(TestItem, oneshot::Sender<TestResult>),
    Reload(Vec<String>, oneshot::Sender<()>),
    Shutdown,
}

pub struct WorkerPool {
    worker_txs: Vec<mpsc::UnboundedSender<WorkerMsg>>,
    next: Arc<AtomicUsize>,
}

impl WorkerPool {
    #[must_use]
    pub fn new(size: usize, python_bin: &str, root: &Path) -> Self {
        let size = size.max(1);
        let mut worker_txs = Vec::with_capacity(size);
        let python_path = vec![root.to_owned()];
        for _ in 0..size {
            let (tx, rx) = mpsc::unbounded_channel();
            let bin = python_bin.to_owned();
            tokio::spawn(worker_task(bin, python_path.clone(), rx));
            worker_txs.push(tx);
        }
        Self {
            worker_txs,
            next: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn run(&self, tests: Vec<TestItem>) -> impl Stream<Item = TestResult> + use<> {
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let n = self.worker_txs.len();
        let next = Arc::clone(&self.next);

        for test in tests {
            let (result_tx, result_rx) = oneshot::channel();
            let idx = next.fetch_add(1, Ordering::Relaxed) % n;
            let _ = self.worker_txs[idx].send(WorkerMsg::Test(test, result_tx));
            let stx = stream_tx.clone();
            tokio::spawn(async move {
                if let Ok(result) = result_rx.await {
                    let _ = stx.send(result);
                }
            });
        }

        UnboundedReceiverStream::new(stream_rx)
    }

    pub async fn reload(&self, modules: Vec<String>) {
        let mut ack_rxs = Vec::with_capacity(self.worker_txs.len());
        for tx in &self.worker_txs {
            let (ack_tx, ack_rx) = oneshot::channel();
            let _ = tx.send(WorkerMsg::Reload(modules.clone(), ack_tx));
            ack_rxs.push(ack_rx);
        }
        for ack_rx in ack_rxs {
            let _ = ack_rx.await;
        }
    }

    pub fn shutdown(self) {
        for tx in self.worker_txs {
            let _ = tx.send(WorkerMsg::Shutdown);
        }
    }
}

// converts a file path to a python module name relative to root
// e.g. /project/tests/test_math.py -> tests.test_math
#[must_use]
pub fn path_to_module(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let without_ext = relative.with_extension("");
    let parts: Vec<String> = without_ext
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("."))
}

async fn worker_task(
    python_bin: String,
    python_path: Vec<std::path::PathBuf>,
    mut rx: mpsc::UnboundedReceiver<WorkerMsg>,
) {
    let path_refs: Vec<&Path> = python_path.iter().map(PathBuf::as_path).collect();
    let mut worker: Option<WorkerProcess> = None;

    while let Some(msg) = rx.recv().await {
        match msg {
            WorkerMsg::Test(test, result_tx) => {
                debug!("worker_task: running test {}", test.name);
                if worker.is_none() {
                    debug!("worker_task: spawning process");
                    match WorkerProcess::spawn(&python_bin, &path_refs) {
                        Ok(w) => worker = Some(w),
                        Err(e) => {
                            let msg = format!("worker spawn failed: {e}");
                            debug!("worker_task: {msg}");
                            let _ = result_tx.send(TestResult {
                                test,
                                outcome: TestOutcome::Error { message: msg },
                                duration: Duration::ZERO,
                                stdout: String::new(),
                                stderr: String::new(),
                            });
                            continue;
                        }
                    }
                }
                let Some(w) = worker.as_mut() else {
                    unreachable!("worker is always Some after the spawn block above");
                };
                match w.run_test(&test).await {
                    Ok(result) => {
                        debug!("worker_task: test {} done", test.name);
                        let _ = result_tx.send(result);
                    }
                    Err(first_err) => {
                        debug!("worker_task: run_test error, respawning for retry");
                        let stderr_output = w.drain_stderr().await;
                        worker = WorkerProcess::spawn(&python_bin, &path_refs).ok();
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
            WorkerMsg::Reload(modules, ack_tx) => {
                debug!("worker_task: reload {modules:?}");
                if let Some(w) = worker.as_mut() {
                    let _ = w.reload(&modules).await;
                }
                let _ = ack_tx.send(());
            }
            WorkerMsg::Shutdown => {
                debug!("worker_task: shutdown");
                break;
            }
        }
    }

    if let Some(mut w) = worker {
        w.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn path_to_module_basic() {
        let root = PathBuf::from("/project");
        let path = PathBuf::from("/project/tests/test_math.py");
        assert_eq!(
            path_to_module(&root, &path),
            Some("tests.test_math".to_string())
        );
    }

    #[test]
    fn path_to_module_top_level() {
        let root = PathBuf::from("/project");
        let path = PathBuf::from("/project/test_foo.py");
        assert_eq!(path_to_module(&root, &path), Some("test_foo".to_string()));
    }

    #[test]
    fn path_to_module_not_under_root() {
        let root = PathBuf::from("/project");
        let path = PathBuf::from("/other/test_foo.py");
        assert_eq!(path_to_module(&root, &path), None);
    }

    #[test]
    fn path_to_module_root_itself() {
        let root = PathBuf::from("/project");
        let path = PathBuf::from("/project");
        assert_eq!(path_to_module(&root, &path), None);
    }
}
