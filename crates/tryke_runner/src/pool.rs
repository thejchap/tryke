use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Result, anyhow};
use log::{LevelFilter, warn};

use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_stream::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tryke_config::Project;
use tryke_types::TestResult;

use crate::schedule::WorkUnit;
use crate::worker::{Worker, WorkerCtrl, WorkerMsg};

const WORKER_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

pub struct WorkerPool {
    /// Sender channel for work units
    ///
    /// Workers claim individual units off of this
    work_tx: async_channel::Sender<WorkerMsg>,

    /// Control channels
    ///
    /// One per-worker to distribute messages to all.
    ctrl_txs: Vec<mpsc::UnboundedSender<WorkerCtrl>>,

    /// Pool-wide shutdown signal, observed ahead of both work and control messages.
    shutdown_tx: watch::Sender<bool>,

    /// Every worker task remains owned by the pool until shutdown completes.
    worker_handles: Vec<JoinHandle<()>>,
}

#[must_use = "dropping a worker run cancels its submitted work"]
pub struct WorkerRun {
    results: UnboundedReceiverStream<TestResult>,
    cancel_tx: watch::Sender<bool>,
}

impl Stream for WorkerRun {
    type Item = TestResult;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Pin::new(&mut this.results).poll_next(cx)
    }
}

impl Drop for WorkerRun {
    fn drop(&mut self) {
        let _ = self.cancel_tx.send(true);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WorkerPoolOptions<'a> {
    pub size: usize,
    pub python_path: Option<&'a [PathBuf]>,
    pub log_level: LevelFilter,
    pub warm: bool,
}

impl WorkerPool {
    pub async fn spawn(project: &Project, options: WorkerPoolOptions<'_>) -> Self {
        Self::spawn_from_parts(
            options.size,
            project.python(),
            project.root(),
            options.python_path,
            options.log_level,
            options.warm,
        )
        .await
    }

    pub async fn spawn_from_parts(
        size: usize,
        python_bin: &str,
        root: &Path,
        python_path: Option<&[PathBuf]>,
        log_level: LevelFilter,
        warm: bool,
    ) -> Self {
        let size = size.max(1);
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let python_bin = python_bin.to_owned();
        let python_path = python_path.map_or_else(
            || {
                let mut paths = vec![root.clone()];
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
        let (work_tx, work_rx) = async_channel::unbounded();
        let mut ctrl_txs = Vec::with_capacity(size);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut worker_handles = Vec::with_capacity(size);

        for _ in 0..size {
            let work_rx = work_rx.clone();
            let shutdown_rx = shutdown_rx.clone();
            let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
            ctrl_txs.push(ctrl_tx);
            let worker = Worker::new(
                python_bin.clone(),
                python_path.clone(),
                root.clone(),
                log_level,
            );

            worker_handles.push(tokio::spawn(worker.run(work_rx, ctrl_rx, shutdown_rx)));
        }

        let pool = Self {
            work_tx,
            ctrl_txs,
            shutdown_tx,
            worker_handles,
        };

        if warm {
            pool.warm().await;
        }

        pool
    }

    pub fn submit(&self, units: Vec<WorkUnit>) -> WorkerRun {
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);

        for unit in units {
            let _ = self.work_tx.send_blocking(WorkerMsg::Unit {
                unit,
                result_tx: stream_tx.clone(),
                cancel_rx: cancel_rx.clone(),
            });
        }

        WorkerRun {
            results: UnboundedReceiverStream::new(stream_rx),
            cancel_tx,
        }
    }

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
            warn!(
                "worker control operation '{operation}' timed out after {WORKER_CONTROL_TIMEOUT:?}"
            );
        }
    }

    /// Replace every Python subprocess with a clean, pre-warmed process.
    ///
    /// Hook metadata belongs to work units, so the fresh processes stay
    /// unconfigured until their next unit installs its current registrations.
    pub async fn restart_workers(&self) {
        self.fanout_ctrl("restart", WorkerCtrl::Restart).await;
        self.warm().await;
    }

    async fn warm(&self) {
        self.fanout_ctrl("warm", WorkerCtrl::Ping).await;
    }

    /// Shut down every worker process and await every worker task.
    ///
    /// Workers get one second to terminate cleanly. Tasks still running after
    /// that deadline are aborted and awaited as a cleanup fallback.
    ///
    /// # Errors
    ///
    /// Returns an error if a worker task panicked or otherwise failed to join.
    pub async fn shutdown(mut self) -> Result<()> {
        let _ = self.shutdown_tx.send(true);
        let deadline = tokio::time::Instant::now() + WORKER_SHUTDOWN_TIMEOUT;
        let mut failures = Vec::new();

        for (index, mut handle) in std::mem::take(&mut self.worker_handles)
            .into_iter()
            .enumerate()
        {
            if let Ok(result) = tokio::time::timeout_at(deadline, &mut handle).await {
                record_join_result(index, result, &mut failures);
            } else {
                handle.abort();
                match handle.await {
                    Err(error) if error.is_cancelled() => {}
                    result => record_join_result(index, result, &mut failures),
                }
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(
                "Worker pool shutdown encountered task failures: {}",
                failures.join("; ")
            ))
        }
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        for handle in &self.worker_handles {
            handle.abort();
        }
    }
}

fn record_join_result(
    index: usize,
    result: std::result::Result<(), tokio::task::JoinError>,
    failures: &mut Vec<String>,
) {
    if let Err(error) = result {
        let kind = if error.is_panic() {
            "panicked"
        } else {
            "failed to join"
        };
        failures.push(format!("worker {index} {kind}: {error}"));
    }
}
