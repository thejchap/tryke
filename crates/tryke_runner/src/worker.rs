use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use log::{LevelFilter, debug, trace};
use tokio::sync::{mpsc, oneshot, watch};
use tryke_types::{TestOutcome, TestResult};

use crate::protocol::RegisterHooksParams;
use crate::schedule::WorkUnit;
pub use crate::worker_process::WorkerProcess;

const WORKER_SPAWN_TIMEOUT: Duration = Duration::from_secs(5);

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
        cancel_rx: watch::Receiver<bool>,
    },
}

pub(crate) enum WorkerCtrl {
    Ping(oneshot::Sender<()>),
    Restart(oneshot::Sender<()>),
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
        let python_bin = self.python_bin.clone();
        let python_paths = self.python_path.clone();
        let root = self.root.clone();
        let log_level = self.log_level;
        let spawn = tokio::task::spawn_blocking(move || {
            let path_refs = python_paths
                .iter()
                .map(PathBuf::as_path)
                .collect::<Vec<_>>();
            WorkerProcess::spawn(&python_bin, &path_refs, &root, log_level)
        });

        match tokio::time::timeout(WORKER_SPAWN_TIMEOUT, spawn).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(anyhow!("Worker spawn task failed: {error}")),
            Err(_) => Err(anyhow!(
                "Worker process spawn timed out after {WORKER_SPAWN_TIMEOUT:?}"
            )),
        }
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
                .unwrap_or_else(|| "Worker unavailable (spawn or hook replay failed)".into());

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

    async fn handle_unit(&mut self, unit: WorkUnit, result_tx: mpsc::UnboundedSender<TestResult>) {
        let registrations = Self::registrations_for_unit(&unit);

        self.prepare_unit(&registrations).await;

        for test in unit.tests {
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

    async fn handle_control(&mut self, ctrl: WorkerCtrl) {
        match ctrl {
            WorkerCtrl::Ping(ack_tx) => {
                trace!("Worker: ping (pre-warm)");
                let _ = self.ensure_process(&[]).await;
                let _ = ack_tx.send(());
            }
            WorkerCtrl::Restart(ack_tx) => {
                trace!("Worker: restart");
                self.reset_process().await;
                let _ = ack_tx.send(());
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

    pub(crate) async fn run(
        mut self,
        work_rx: async_channel::Receiver<WorkerMsg>,
        mut ctrl_rx: mpsc::UnboundedReceiver<WorkerCtrl>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        'worker: loop {
            tokio::select! {
                biased;
                () = wait_for_signal(&mut shutdown_rx) => break,
                ctrl = ctrl_rx.recv() => {
                    let Some(ctrl) = ctrl else { break };
                    self.handle_control(ctrl).await;
                }
                msg = work_rx.recv() => {
                    match msg {
                        Ok(WorkerMsg::Unit {
                            unit,
                            result_tx,
                            mut cancel_rx,
                        }) => {
                            if *cancel_rx.borrow() {
                                continue;
                            }

                            let interrupted_by_shutdown = {
                                let unit_future = self.handle_unit(unit, result_tx);

                                tokio::pin!(unit_future);

                                tokio::select! {
                                    biased;
                                    () = wait_for_signal(&mut shutdown_rx) => Some(true),
                                    () = wait_for_signal(&mut cancel_rx) => Some(false),
                                    () = &mut unit_future => None,
                                }
                            };

                            if let Some(shutdown) = interrupted_by_shutdown {
                                self.reset_process().await;
                                if shutdown {
                                    break 'worker;
                                }
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

async fn wait_for_signal(signal_rx: &mut watch::Receiver<bool>) {
    if *signal_rx.borrow() {
        return;
    }
    while signal_rx.changed().await.is_ok() {
        if *signal_rx.borrow_and_update() {
            return;
        }
    }
}
