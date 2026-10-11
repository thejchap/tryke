use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use log::{LevelFilter, debug, trace};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tryke_types::{TestOutcome, TestResult};

use crate::protocol::RegisterHooksParams;
use crate::schedule::WorkUnit;
use crate::worker_process::WorkerProcess;

const WORKER_UNAVAILABLE: &str = "Worker unavailable (spawn or hook replay failed)";

/// One logical worker slot, which may replace its Python subprocess after a
/// crash, cancellation, or restart.
pub(crate) struct Worker {
    python_bin: String,
    python_path: Vec<PathBuf>,
    root: PathBuf,
    log_level: LevelFilter,
    process: Option<WorkerProcess>,
    /// Most recent spawn or hook-replay failure, captured so
    /// `run_single_test` can surface the real reason (and any worker
    /// stderr) instead of the opaque "worker unavailable" placeholder.
    /// Cleared once we have a live worker again so we don't replay a
    /// stale error against an unrelated test.
    last_failure: Option<String>,
}

pub(crate) enum WorkerMsg {
    Unit {
        unit: WorkUnit,
        result_tx: mpsc::UnboundedSender<TestResult>,
        cancel: CancellationToken,
    },
}

pub(crate) enum WorkerCtrl {
    Ping(oneshot::Sender<Result<()>>),
    Restart(oneshot::Sender<Result<()>>),
}

fn format_worker_failure(prefix: &str, error: &dyn std::fmt::Display, stderr: &str) -> String {
    let mut message = format!("{prefix}: {error}");
    let trimmed = stderr.trim();
    if !trimmed.is_empty() {
        message.push_str("\nWorker stderr:\n");
        message.push_str(trimmed);
    }
    message
}

impl Worker {
    pub(crate) fn new(
        python_bin: String,
        python_path: Vec<PathBuf>,
        root: PathBuf,
        log_level: LevelFilter,
    ) -> Self {
        Self {
            python_bin,
            python_path,
            root,
            log_level,
            process: None,
            last_failure: None,
        }
    }

    async fn spawn_process(&self) -> Result<WorkerProcess> {
        let path_refs = self
            .python_path
            .iter()
            .map(PathBuf::as_path)
            .collect::<Vec<_>>();
        WorkerProcess::spawn(&self.python_bin, &path_refs, &self.root, self.log_level).await
    }

    /// Ensure a Python process is live, replaying only the active unit's hook
    /// registrations when a replacement process is needed.
    async fn ensure_process<'a>(
        &'a mut self,
        registrations: &[RegisterHooksParams],
    ) -> Option<&'a mut WorkerProcess> {
        if self.process.is_some() {
            return self.process.as_mut();
        }

        trace!("Worker: spawning process");

        let mut process = match self.spawn_process().await {
            Ok(process) => process,
            Err(error) => {
                let message = format_worker_failure(
                    &format!(
                        "Failed to spawn Python worker ({} -m tryke.worker)",
                        self.python_bin
                    ),
                    &error,
                    "",
                );

                debug!("Worker: {message}");

                self.last_failure = Some(message);

                return None;
            }
        };

        for params in registrations {
            if let Err(error) = process.register_hooks(params.clone()).await {
                let stderr_output = process.drain_stderr().await;

                let message = format_worker_failure(
                    &format!("Hook replay failed for module {}", params.module),
                    &error,
                    &stderr_output,
                );

                debug!("Worker: {message}");

                self.last_failure = Some(message);

                return None;
            }
        }

        self.last_failure = None;
        self.process = Some(process);
        self.process.as_mut()
    }

    /// Install the active unit's complete hook metadata on an existing
    /// process, or spawn a process and replay it there.
    async fn prepare_unit(&mut self, registrations: &[RegisterHooksParams]) {
        if self.process.is_none() {
            let _ = self.ensure_process(registrations).await;
            return;
        }

        for params in registrations {
            let registration_failure = {
                let Some(process) = self.process.as_mut() else {
                    return;
                };

                match process.register_hooks(params.clone()).await {
                    Ok(()) => None,
                    Err(error) => {
                        let stderr_output = process.drain_stderr().await;
                        Some((error, stderr_output))
                    }
                }
            };

            if let Some((error, stderr_output)) = registration_failure {
                let message = format_worker_failure(
                    &format!("Hook registration failed for module {}", params.module),
                    &error,
                    &stderr_output,
                );

                debug!("Worker: {message}");

                self.last_failure = Some(message);
                self.process = None;

                return;
            }
        }

        self.last_failure = None;
    }

    async fn run_single_test(
        &mut self,
        test: tryke_types::TestItem,
        registrations: &[RegisterHooksParams],
        result_tx: &mpsc::UnboundedSender<TestResult>,
    ) {
        let Some(process) = self.ensure_process(registrations).await else {
            let message = self
                .last_failure
                .clone()
                .unwrap_or_else(|| WORKER_UNAVAILABLE.into());

            let _ = result_tx.send(TestResult {
                test,
                outcome: TestOutcome::Error { message },
                duration: Duration::ZERO,
                stdout: String::new(),
                stderr: String::new(),
            });

            return;
        };

        match process.run_test(&test).await {
            Ok(result) => {
                trace!("Worker: test {} done", test.name);
                let _ = result_tx.send(result);
            }
            Err(error) => {
                debug!("Worker: run_test error for {}: {error}", test.name);
                let stderr_output = process.drain_stderr().await;

                // The next test reconstructs a replacement process from this
                // active unit's registrations.
                self.process = None;

                let message = format_worker_failure("Worker error", &error, &stderr_output);

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

    fn registrations_for_unit(unit: &WorkUnit) -> Vec<RegisterHooksParams> {
        let mut seen = std::collections::HashSet::new();
        unit.tests
            .iter()
            .filter(|test| seen.insert(test.module_path.clone()))
            .map(|test| RegisterHooksParams::for_test(test, &unit.hooks))
            .collect()
    }

    async fn handle_unit(
        &mut self,
        unit: WorkUnit,
        result_tx: mpsc::UnboundedSender<TestResult>,
        cancel: &CancellationToken,
    ) {
        let registrations = Self::registrations_for_unit(&unit);

        self.prepare_unit(&registrations).await;

        for test in unit.tests {
            // A cancelled run stops between tests rather than interrupting
            // one: cutting an RPC short would leave the interpreter unusable
            // and skip the `per="scope"` teardown below.
            if cancel.is_cancelled() {
                break;
            }

            trace!("Worker: running test {}", test.name);

            self.run_single_test(test, &registrations, &result_tx).await;
        }

        for params in registrations {
            if let Some(process) = self.process.as_mut()
                && let Err(error) = process.finalize_hooks(params.module).await
            {
                debug!("Worker: finalize_hooks failed: {error}");
            }
        }
    }

    /// Ensure a Python process is live and has answered a `ping` with
    /// `pong`. A process that fails the ping is discarded.
    async fn warm_process(&mut self) -> Result<()> {
        let Some(process) = self.ensure_process(&[]).await else {
            let message = self
                .last_failure
                .clone()
                .unwrap_or_else(|| WORKER_UNAVAILABLE.into());
            return Err(anyhow!(message));
        };

        let Err(error) = process.ping().await else {
            return Ok(());
        };
        let stderr_output = process.drain_stderr().await;
        let message = format_worker_failure("Worker ping failed", &error, &stderr_output);

        debug!("Worker: {message}");

        self.last_failure = Some(message.clone());
        self.process = None;

        Err(anyhow!(message))
    }

    async fn handle_control(&mut self, ctrl: WorkerCtrl) {
        match ctrl {
            WorkerCtrl::Ping(mut ack_tx) => {
                trace!("Worker: ping (pre-warm)");
                let warmed = {
                    let warm = self.warm_process();
                    tokio::select! {
                        biased;
                        () = ack_tx.closed() => None,
                        result = warm => Some(result),
                    }
                };
                if let Some(result) = warmed {
                    let _ = ack_tx.send(result);
                } else {
                    // The pool stopped waiting mid-warm. An interrupted RPC
                    // may leave a half-exchanged request on the interpreter's
                    // pipes, so the process is not reusable.
                    trace!("Worker: warm abandoned; discarding process");
                    self.reset_process().await;
                }
            }
            WorkerCtrl::Restart(ack_tx) => {
                trace!("Worker: restart");
                self.reset_process().await;
                let _ = ack_tx.send(Ok(()));
            }
        }
    }

    async fn reset_process(&mut self) {
        if let Some(mut process) = self.process.take() {
            process.shutdown().await;
        }
    }

    async fn shutdown(mut self) {
        self.reset_process().await;
    }

    /// Claim work units until the pool shuts down or its channels close.
    ///
    /// `async_channel::Receiver::recv` is polled inside `select!` and is
    /// therefore dropped whenever the shutdown or control branch wins. That is
    /// safe with async-channel 2.x — a dropped `Recv` re-notifies another
    /// listener rather than swallowing the unit — but unlike
    /// `tokio::sync::mpsc` the crate does not document the guarantee, so a
    /// channel swap here needs to re-check it.
    pub(crate) async fn run(
        mut self,
        work_rx: async_channel::Receiver<WorkerMsg>,
        mut ctrl_rx: mpsc::UnboundedReceiver<WorkerCtrl>,
        shutdown: CancellationToken,
    ) {
        'worker: loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                ctrl = ctrl_rx.recv() => {
                    let Some(ctrl) = ctrl else { break };
                    // A stalled warm must not hold the worker past shutdown;
                    // the final `shutdown` below discards its process.
                    tokio::select! {
                        biased;
                        () = shutdown.cancelled() => break 'worker,
                        () = self.handle_control(ctrl) => {}
                    }
                }
                msg = work_rx.recv() => {
                    match msg {
                        Ok(WorkerMsg::Unit {
                            unit,
                            result_tx,
                            cancel,
                        }) => {
                            if cancel.is_cancelled() {
                                continue;
                            }

                            let interrupted_by_shutdown = {
                                let unit_future = self.handle_unit(unit, result_tx, &cancel);

                                tokio::pin!(unit_future);

                                tokio::select! {
                                    biased;
                                    () = shutdown.cancelled() => true,
                                    () = &mut unit_future => false,
                                }
                            };

                            if interrupted_by_shutdown {
                                // The interrupted unit left an RPC half-written
                                // on the interpreter's stdin, so the process is
                                // no longer usable.
                                self.reset_process().await;
                                break 'worker;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }

        self.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::pin::Pin;

    use tokio::sync::oneshot::error::TryRecvError;
    use tryke_testing::{TestProject, python_bin, workspace_root};

    use super::*;

    const WATCHDOG: Duration = Duration::from_secs(30);

    /// Interpreter startup hook that records the worker PID in
    /// `worker-started`, then holds startup until `worker-release` exists.
    /// The deadline keeps a broken test from wedging the interpreter forever.
    const GATED_SITECUSTOMIZE: &str = "\
import os, pathlib, time
root = pathlib.Path(__file__).parent
tmp = root / f'worker-started.{os.getpid()}'
tmp.write_text(str(os.getpid()))
os.replace(tmp, root / 'worker-started')
deadline = time.monotonic() + 30
while not (root / 'worker-release').exists() and time.monotonic() < deadline:
    time.sleep(0.01)
";

    fn gated_project() -> TestProject {
        TestProject::with_files([("sitecustomize.py", GATED_SITECUSTOMIZE)])
            .expect("create gated test project")
    }

    fn test_worker(project: &TestProject) -> Worker {
        Worker::new(
            python_bin(),
            vec![
                project.root().to_path_buf(),
                workspace_root().join("python"),
            ],
            project.root().to_path_buf(),
            LevelFilter::Off,
        )
    }

    fn started_pid(project: &TestProject) -> Option<u32> {
        std::fs::read_to_string(project.root().join("worker-started"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn release(project: &TestProject) {
        std::fs::write(project.root().join("worker-release"), "").expect("release worker startup");
    }

    /// Drive `control` until the gated interpreter has started. Returns
    /// `None` if `control` finished first or the watchdog expired.
    async fn poll_until_started<F: Future<Output = ()>>(
        control: &mut Pin<&mut F>,
        project: &TestProject,
    ) -> Option<u32> {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                tokio::select! {
                    biased;
                    () = control.as_mut() => return None,
                    () = tokio::time::sleep(Duration::from_millis(10)) => {}
                }
                if let Some(pid) = started_pid(project) {
                    return Some(pid);
                }
            }
        })
        .await
        .ok()
        .flatten()
    }

    async fn wait_for_marker(marker: &Path) -> bool {
        tokio::time::timeout(WATCHDOG, async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn warm_acknowledgement_waits_for_ready_worker() {
        let project = gated_project();
        let mut worker = test_worker(&project);
        let (ack_tx, mut ack_rx) = oneshot::channel();

        let (started, pending_while_gated, finished) = {
            let control = worker.handle_control(WorkerCtrl::Ping(ack_tx));
            tokio::pin!(control);
            let started = poll_until_started(&mut control, &project).await;
            let pending_while_gated = matches!(ack_rx.try_recv(), Err(TryRecvError::Empty));
            release(&project);
            let finished = tokio::time::timeout(WATCHDOG, control).await.is_ok();
            (started, pending_while_gated, finished)
        };
        let acknowledgement = ack_rx.await;
        worker.shutdown().await;

        assert!(started.is_some(), "gated worker should start");
        assert!(pending_while_gated, "warm must not ack before pong");
        assert!(finished, "warm should finish after release");
        assert!(
            matches!(acknowledgement, Ok(Ok(()))),
            "got {acknowledgement:?}"
        );
    }

    #[tokio::test]
    async fn cancelled_warm_discards_process() {
        let project = gated_project();
        let mut worker = test_worker(&project);
        let (ack_tx, ack_rx) = oneshot::channel();

        let (first_pid, cleaned_up) = {
            let control = worker.handle_control(WorkerCtrl::Ping(ack_tx));
            tokio::pin!(control);
            let first_pid = poll_until_started(&mut control, &project).await;
            drop(ack_rx);
            // Startup is still gated, so completion here means cleanup ran.
            let cleaned_up = tokio::time::timeout(WATCHDOG, control).await.is_ok();
            (first_pid, cleaned_up)
        };
        let discarded = worker.process.is_none();

        std::fs::remove_file(project.root().join("worker-started")).expect("clear start marker");
        release(&project);
        let (ack_tx, ack_rx) = oneshot::channel();
        let rewarmed =
            tokio::time::timeout(WATCHDOG, worker.handle_control(WorkerCtrl::Ping(ack_tx)))
                .await
                .is_ok();
        let acknowledgement = ack_rx.await;
        let second_pid = started_pid(&project);
        worker.shutdown().await;

        assert!(first_pid.is_some(), "gated worker should start");
        assert!(cleaned_up, "cancelled warm should finish its cleanup");
        assert!(discarded, "interrupted interpreter must be discarded");
        assert!(rewarmed, "second warm should finish");
        assert!(
            matches!(acknowledgement, Ok(Ok(()))),
            "got {acknowledgement:?}"
        );
        assert!(second_pid.is_some(), "second warm should start a worker");
        assert_ne!(first_pid, second_pid, "a new interpreter must be started");
    }

    #[tokio::test]
    async fn shutdown_interrupts_worker_warming() {
        let project = gated_project();
        let worker = test_worker(&project);
        let (_work_tx, work_rx) = async_channel::unbounded();
        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(worker.run(work_rx, ctrl_rx, shutdown.clone()));
        let (ack_tx, ack_rx) = oneshot::channel();

        let sent = ctrl_tx.send(WorkerCtrl::Ping(ack_tx)).is_ok();
        let started = wait_for_marker(&project.root().join("worker-started")).await;
        shutdown.cancel();
        let joined = tokio::time::timeout(WATCHDOG, task).await;
        let acknowledgement = ack_rx.await;

        assert!(sent, "worker should accept the ping");
        assert!(started, "gated worker should start");
        assert!(
            matches!(joined, Ok(Ok(()))),
            "worker should stop on its own: {joined:?}"
        );
        assert!(
            acknowledgement.is_err(),
            "an interrupted warm must not acknowledge readiness"
        );
    }
}
