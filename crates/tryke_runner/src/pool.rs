use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Result, anyhow};
use log::{LevelFilter, warn};

use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinError, JoinSet};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;
use tryke_config::Project;
use tryke_types::TestResult;

use crate::schedule::WorkUnit;
use crate::worker::{Worker, WorkerCtrl, WorkerMsg};

const WORKER_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
enum WorkerControlOperation {
    Restart,
    Warm,
}

impl WorkerControlOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Warm => "warm",
        }
    }

    fn message(self, ack: oneshot::Sender<Result<()>>) -> WorkerCtrl {
        match self {
            Self::Restart => WorkerCtrl::Restart(ack),
            Self::Warm => WorkerCtrl::Ping(ack),
        }
    }
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

    /// Pool-wide shutdown signal, observed ahead of both work and control messages.
    shutdown: CancellationToken,

    /// Every worker task remains owned by the pool until shutdown completes.
    ///
    /// A `JoinSet` rather than `tokio_util`'s `TaskTracker`: the pool is a
    /// fixed-size set of tasks whose `JoinError`s we want to report, and
    /// dropping a `JoinSet` aborts its tasks. `TaskTracker` discards task
    /// outcomes, which would turn a panicking worker into a silently short
    /// run instead of a shutdown error.
    workers: JoinSet<()>,
}

#[must_use = "dropping a worker run cancels its submitted work"]
pub struct WorkerRun {
    results: UnboundedReceiverStream<TestResult>,
    cancel: CancellationToken,
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
        self.cancel.cancel();
    }
}

impl WorkerRun {
    /// Stops the run without starting further tests.
    ///
    /// Units already on a worker finish their current test and their
    /// `per="scope"` fixture teardown; this waits for that and discards the
    /// remaining results, so the pool is idle when it returns.
    pub async fn stop(&mut self) {
        self.cancel.cancel();
        while self.results.next().await.is_some() {}
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
        let shutdown = CancellationToken::new();
        let mut workers = JoinSet::new();

        for _ in 0..size {
            let work_rx = work_rx.clone();
            let shutdown = shutdown.clone();
            let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
            ctrl_txs.push(ctrl_tx);
            let worker = Worker::new(
                python_bin.clone(),
                python_path.clone(),
                root.clone(),
                log_level,
            );

            workers.spawn(worker.run(work_rx, ctrl_rx, shutdown));
        }

        let pool = Self {
            work_tx,
            ctrl_txs,
            shutdown,
            workers,
        };

        // Pre-warming is best-effort: a worker that misses the deadline just
        // pays interpreter startup on its first unit. Unlike a missed restart
        // it cannot leave stale code behind, so it must not fail construction.
        if warm && let Err(error) = pool.warm().await {
            warn!("{error:#}");
        }

        pool
    }

    /// Queues `units` for the workers and streams their results.
    ///
    /// # Errors
    ///
    /// Returns an error if every worker task has exited, because no queued
    /// unit would ever produce a result.
    pub fn submit(&self, units: Vec<WorkUnit>) -> Result<WorkerRun> {
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();

        for unit in units {
            // `try_send`, not `send_blocking`: every caller is async, and
            // async-channel documents `send_blocking` as deadlock-prone in an
            // async context. On an unbounded channel the only failure is a
            // closed channel, which means no worker task is left to run it.
            let sent = self.work_tx.try_send(WorkerMsg::Unit {
                unit,
                result_tx: stream_tx.clone(),
                cancel: cancel.clone(),
            });
            if sent.is_err() {
                cancel.cancel();
                return Err(anyhow!("worker pool has no running workers"));
            }
        }

        Ok(WorkerRun {
            results: UnboundedReceiverStream::new(stream_rx),
            cancel,
        })
    }

    /// Send `operation`'s control message to every worker and collect every
    /// acknowledgement against one shared deadline.
    ///
    /// `timeout` is a parameter rather than a direct read of
    /// [`WORKER_CONTROL_TIMEOUT`] so the failure path stays testable without a
    /// five-second wall-clock wait.
    async fn fanout_ctrl(
        &self,
        operation: WorkerControlOperation,
        timeout: Duration,
    ) -> Result<()> {
        const DEPARTED_BEFORE_READY: &str = "worker task exited before readiness acknowledgement";

        let mut pending = Vec::with_capacity(self.ctrl_txs.len());
        let mut failures = Vec::new();
        for (index, ctrl_tx) in self.ctrl_txs.iter().enumerate() {
            let (ack_tx, ack_rx) = oneshot::channel();
            if ctrl_tx.send(operation.message(ack_tx)).is_ok() {
                pending.push((index, ack_rx));
            } else if let WorkerControlOperation::Warm = operation {
                failures.push(format!("worker {index}: {DEPARTED_BEFORE_READY}"));
            }
        }

        // One deadline for the whole fan-out: the messages are already in
        // flight, so awaiting the acks in sequence costs no extra wall time.
        // A timed-out receiver is dropped, which tells a still-running warm to
        // abandon and discard its interpreter.
        let deadline = tokio::time::Instant::now() + timeout;
        let mut unacked = Vec::new();
        for (index, ack_rx) in pending {
            match tokio::time::timeout_at(deadline, ack_rx).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(error))) => failures.push(format!("worker {index}: {error:#}")),
                // A dropped ack sender means the worker task exited and killed
                // its interpreter on the way out. Nothing stale is left behind
                // for a restart, but no ready interpreter exists for a warm.
                Ok(Err(_)) => match operation {
                    WorkerControlOperation::Restart => {}
                    WorkerControlOperation::Warm => {
                        failures.push(format!("worker {index}: {DEPARTED_BEFORE_READY}"));
                    }
                },
                Err(_) => unacked.push(index),
            }
        }

        let name = operation.name();
        if unacked.is_empty() {
            if failures.is_empty() {
                return Ok(());
            }
            return Err(anyhow!(
                "worker control operation '{name}' failed: {}",
                failures.join("; ")
            ));
        }

        let mut message = format!(
            "worker control operation '{name}' timed out after {timeout:?}; \
             {}/{} workers did not acknowledge (workers {unacked:?})",
            unacked.len(),
            self.ctrl_txs.len(),
        );
        if !failures.is_empty() {
            let _ = write!(message, "; worker failures: {}", failures.join("; "));
        }
        Err(anyhow!(message))
    }

    /// Replace every Python subprocess with a clean, pre-warmed process.
    ///
    /// Hook metadata belongs to work units, so the fresh processes stay
    /// unconfigured until their next unit installs its current registrations.
    ///
    /// # Errors
    ///
    /// Returns an error if a worker fails to acknowledge the restart within
    /// [`WORKER_CONTROL_TIMEOUT`]. A worker only misses that deadline while it
    /// is still executing a unit, which means it is still holding the *old*
    /// interpreter — so the caller must not present the next run's results as
    /// reflecting current source.
    pub async fn restart_workers(&self) -> Result<()> {
        self.fanout_ctrl(WorkerControlOperation::Restart, WORKER_CONTROL_TIMEOUT)
            .await?;

        // Warming is an optimization, not a correctness guarantee — see the
        // note in `spawn_from_parts`.
        if let Err(error) = self.warm().await {
            warn!("{error:#}");
        }

        Ok(())
    }

    /// Start every worker's interpreter and wait for each to answer `ping`.
    async fn warm(&self) -> Result<()> {
        self.fanout_ctrl(WorkerControlOperation::Warm, WORKER_CONTROL_TIMEOUT)
            .await
    }

    /// Shut down every worker process and await every worker task.
    ///
    /// Workers get one second *in total* to terminate cleanly — they stop
    /// concurrently, so a per-worker budget would multiply the wait by the
    /// pool size. Tasks still running past that deadline are aborted.
    ///
    /// # Errors
    ///
    /// Returns an error if a worker task panicked, or if any task had to be
    /// aborted because it outlived the shutdown deadline.
    pub async fn shutdown(mut self) -> Result<()> {
        self.shutdown.cancel();

        let mut workers = std::mem::take(&mut self.workers);
        let mut failures = Vec::new();

        let drained = tokio::time::timeout(WORKER_SHUTDOWN_TIMEOUT, async {
            while let Some(result) = workers.join_next().await {
                record_join_result(result, &mut failures);
            }
        })
        .await;

        if drained.is_err() {
            // Anything still alive is wedged (most likely in Python teardown).
            // `shutdown` aborts the remainder and awaits the aborts, so the
            // kill-on-drop child processes are killed before we return rather
            // than outliving the pool.
            let remaining = workers.len();
            workers.shutdown().await;
            failures.push(format!(
                "{remaining} worker task(s) did not stop within \
                 {WORKER_SHUTDOWN_TIMEOUT:?} and were aborted"
            ));
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
        // Dropping `workers` aborts any task still running.
        self.shutdown.cancel();
    }
}

fn record_join_result(result: std::result::Result<(), JoinError>, failures: &mut Vec<String>) {
    if let Err(error) = result {
        let kind = if error.is_panic() {
            "panicked"
        } else {
            "failed to join"
        };
        failures.push(format!("worker task {kind}: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use log::LevelFilter;
    use tryke_testing::{TestProject, python_bin as test_python_bin, workspace_root};
    use tryke_types::TestItem;

    use super::*;
    use crate::schedule::{DistMode, partition_with_hooks};

    fn python_package_dir() -> PathBuf {
        workspace_root().join("python")
    }

    /// A pool wired up with real control channels but no worker tasks, so the
    /// control plane can be exercised without paying for interpreter startup.
    ///
    /// The returned receivers must be kept alive by the caller: dropping a
    /// `ctrl_rx` closes its channel, which `fanout_ctrl` treats as "worker
    /// already gone" rather than as a missed acknowledgement.
    fn control_only_pool(
        size: usize,
    ) -> (
        WorkerPool,
        async_channel::Receiver<WorkerMsg>,
        Vec<mpsc::UnboundedReceiver<WorkerCtrl>>,
    ) {
        let (work_tx, work_rx) = async_channel::unbounded();
        let mut senders = Vec::with_capacity(size);
        let mut receivers = Vec::with_capacity(size);
        for _ in 0..size {
            let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
            senders.push(ctrl_tx);
            receivers.push(ctrl_rx);
        }

        let pool = WorkerPool {
            work_tx,
            ctrl_txs: senders,
            shutdown: CancellationToken::new(),
            workers: JoinSet::new(),
        };

        (pool, work_rx, receivers)
    }

    #[tokio::test]
    async fn fanout_ctrl_reports_every_worker_that_never_acks() {
        let (pool, _work_rx, _ctrl_rxs) = control_only_pool(3);

        let error = pool
            .fanout_ctrl(WorkerControlOperation::Restart, Duration::from_millis(10))
            .await
            .expect_err("silent workers must fail the operation");

        let message = format!("{error:#}");
        assert!(
            message.contains("3/3 workers did not acknowledge (workers [0, 1, 2])"),
            "a silent worker must be identified, not just counted: {message}"
        );
    }

    #[tokio::test]
    async fn fanout_ctrl_treats_a_departed_worker_as_acknowledged() {
        // A closed control channel means the worker task exited and killed its
        // interpreter on the way out. Nothing stale survives it, so it must not
        // be reported as a control failure.
        let (pool, _work_rx, ctrl_rxs) = control_only_pool(2);
        drop(ctrl_rxs);

        pool.fanout_ctrl(WorkerControlOperation::Restart, Duration::from_millis(10))
            .await
            .expect("a departed worker holds no stale interpreter");
    }

    #[tokio::test]
    async fn warm_reports_python_spawn_failure() {
        let project = TestProject::new().expect("create test project");
        let missing_python = project.root().join("missing-python");
        let python_path = [project.root().to_path_buf(), python_package_dir()];
        let pool = WorkerPool::spawn_from_parts(
            1,
            &missing_python.to_string_lossy(),
            project.root(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;

        let warmed = pool.warm().await;
        pool.shutdown().await.expect("clean shutdown");

        let message = format!(
            "{:#}",
            warmed.expect_err("a missing interpreter is not ready")
        );
        assert!(
            message.contains("Failed to spawn Python worker"),
            "{message}"
        );
    }

    /// `restart_workers` on a cold pool must start every process and
    /// acknowledge within the control timeout. This matters because the file
    /// watcher can fire before the user triggers any test run.
    #[tokio::test]
    async fn restart_workers_with_no_live_processes_acks() {
        let project = TestProject::new().expect("create test project");
        let python_path = [project.root().to_path_buf(), python_package_dir()];
        let pool = WorkerPool::spawn_from_parts(
            2,
            &test_python_bin(),
            project.root(),
            Some(&python_path),
            LevelFilter::Off,
            false,
        )
        .await;

        pool.restart_workers()
            .await
            .expect("restart_workers must ack within the control timeout");

        pool.shutdown().await.expect("clean shutdown");
    }

    #[tokio::test]
    async fn shutdown_joins_every_warmed_worker() {
        let project = TestProject::new().expect("create test project");
        let python_path = [project.root().to_path_buf(), python_package_dir()];
        let pool = WorkerPool::spawn_from_parts(
            2,
            &test_python_bin(),
            project.root(),
            Some(&python_path),
            LevelFilter::Off,
            true,
        )
        .await;

        pool.shutdown()
            .await
            .expect("warmed workers must join cleanly");
    }

    #[tokio::test]
    async fn submit_to_a_closed_pool_fails() {
        let (pool, work_rx, _ctrl_rxs) = control_only_pool(1);
        drop(work_rx);
        pool.work_tx.close();

        let units = partition_with_hooks(vec![TestItem::default()], &[], DistMode::Test).units;
        assert!(!units.is_empty(), "test setup should produce a work unit");

        let error = pool
            .submit(units)
            .err()
            .expect("a pool without workers must not report an empty run");
        assert!(
            error.to_string().contains("no running workers"),
            "{error:#}"
        );
    }
}
