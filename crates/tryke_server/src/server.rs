use std::sync::Arc;

use anyhow::Context as _;
use log::debug;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tryke_discovery::Discoverer;
use tryke_runner::WorkerPool;
use tryke_watcher::FileWatcher;

use crate::handler::{
    ConnectionHandler, RunDispatcher, discover_change, notify_change, with_discoverer,
};

enum SessionExit {
    Cancelled,
    Handler(anyhow::Result<()>),
    Dispatcher(Result<anyhow::Result<()>, tokio::task::JoinError>),
}

pub struct Server {
    worker_pool: WorkerPool,
    discoverer: Discoverer,
}

impl Server {
    #[must_use]
    pub fn new(worker_pool: WorkerPool, discoverer: Discoverer) -> Self {
        Self {
            worker_pool,
            discoverer,
        }
    }

    /// Runs the server until the client closes its input and its queued runs
    /// finish, or until cancellation is requested.
    ///
    /// # Errors
    /// Returns an error if file watching cannot be initialized, the client
    /// transport fails, or a server task cannot shut down cleanly.
    pub async fn serve_with_cancellation(
        self,
        cancellation: CancellationToken,
    ) -> anyhow::Result<()> {
        let Self {
            worker_pool,
            discoverer,
        } = self;

        let root = discoverer.root().to_path_buf();
        let excludes = discoverer.excludes().to_vec();
        let lifecycle = cancellation.child_token();

        // Everything sent to the client goes through this queue.
        let (outbound_tx, outbound_rx) = mpsc::channel(256);

        // Initialize the discoverer and populate its import graph and test cache.
        let discoverer = Arc::new(Mutex::new(discoverer));
        let initial_discovery = lifecycle
            .run_until_cancelled(with_discoverer(&discoverer, Discoverer::rediscover))
            .await;
        match initial_discovery {
            None => return worker_pool.shutdown().await,
            Some(Err(error)) => return shutdown_after_startup_error(worker_pool, error).await,
            Some(Ok(_)) => {}
        }

        let watcher = match FileWatcher::spawn(&root, &excludes) {
            Ok(watcher) => watcher,
            Err(error) => return shutdown_after_startup_error(worker_pool, error).await,
        };
        // The watcher holds a weak sender so it never keeps the writer alive
        // once the client has closed its input and queued runs are done.
        let watcher_task = tokio::spawn(watch_files(
            watcher,
            Arc::clone(&discoverer),
            outbound_tx.downgrade(),
            lifecycle.clone(),
        ));

        let (run_tx, run_rx) = mpsc::channel(256);

        let dispatcher = RunDispatcher::new(
            Arc::clone(&discoverer),
            worker_pool,
            outbound_tx.clone(),
            run_rx,
            lifecycle.clone(),
        );

        let mut dispatcher_task = tokio::spawn(dispatcher.run());

        debug!("Server: session started");

        let handler = ConnectionHandler::new(
            tokio::io::stdin(),
            tokio::io::stdout(),
            Arc::clone(&discoverer),
            outbound_rx,
            outbound_tx,
            run_tx,
            lifecycle.clone(),
        );

        let handler = handler.run();
        tokio::pin!(handler);

        let exit = tokio::select! {
            biased;
            () = lifecycle.cancelled() => SessionExit::Cancelled,
            result = &mut handler => SessionExit::Handler(result),
            result = &mut dispatcher_task => SessionExit::Dispatcher(result),
        };

        // A clean exit means the client closed its input, so the other task
        // finishes accepted runs. A failure stops both immediately.
        let failed = match &exit {
            SessionExit::Cancelled => false,
            SessionExit::Handler(result) => result.is_err(),
            SessionExit::Dispatcher(result) => !matches!(result, Ok(Ok(()))),
        };
        if failed {
            lifecycle.cancel();
        }
        debug!("Server: session stopping");

        let (result, secondary_result, secondary_name) = match exit {
            SessionExit::Cancelled => (
                handler.await,
                dispatcher_task
                    .await
                    .context("Run dispatcher task failed")
                    .and_then(std::convert::identity),
                "Run dispatcher",
            ),
            SessionExit::Handler(result) => (
                result,
                dispatcher_task
                    .await
                    .context("Run dispatcher task failed")
                    .and_then(std::convert::identity),
                "Run dispatcher",
            ),
            SessionExit::Dispatcher(result) => (
                result
                    .context("Run dispatcher task failed")
                    .and_then(std::convert::identity),
                handler.await,
                "Connection handler",
            ),
        };
        let result = combine_results(result, secondary_result, secondary_name);

        // The session is over, so stop file-change notifications.
        lifecycle.cancel();
        let watcher_result = watcher_task.await.context("File watcher task failed");

        combine_results(result, watcher_result, "File watcher")
    }
}

/// Keeps `result`'s error as the primary failure and attaches `secondary`'s.
fn combine_results(
    result: anyhow::Result<()>,
    secondary: anyhow::Result<()>,
    secondary_name: &str,
) -> anyhow::Result<()> {
    match (result, secondary) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(secondary_error)) => {
            Err(error.context(format!("{secondary_name} also failed: {secondary_error:#}")))
        }
    }
}

/// Shuts down the worker pool after a startup failure, keeping `error` as the
/// primary failure.
async fn shutdown_after_startup_error(
    worker_pool: WorkerPool,
    error: anyhow::Error,
) -> anyhow::Result<()> {
    match worker_pool.shutdown().await {
        Ok(()) => Err(error),
        Err(shutdown_error) => Err(error.context(format!(
            "Worker pool shutdown also failed: {shutdown_error:#}"
        ))),
    }
}

async fn watch_files(
    mut watcher: FileWatcher,
    discoverer: Arc<Mutex<Discoverer>>,
    outbound_tx: mpsc::WeakSender<bytes::Bytes>,
    cancellation: CancellationToken,
) {
    loop {
        let batch = tokio::select! {
            biased;
            () = cancellation.cancelled() => break,
            batch = watcher.next_batch() => batch,
        };
        let batch = match batch {
            Ok(Some(batch)) => batch,
            Ok(None) => break,
            Err(error) => {
                debug!("Server: stopping file watcher: {error}");
                break;
            }
        };
        // Discovery for a large batch can take a while; shutdown must not
        // wait for it.
        let affected = cancellation
            .run_until_cancelled(discover_change(&discoverer, &batch.paths))
            .await;
        let tests = match affected {
            None => break,
            Some(Ok(Some(tests))) => tests,
            Some(Ok(None)) => continue,
            Some(Err(error)) => {
                debug!("Server: stopping file-change notifications: {error}");
                break;
            }
        };
        // Upgrade only to send: a strong sender held during discovery would
        // keep the writer open after the client closes its input.
        let Some(outbound_tx) = outbound_tx.upgrade() else {
            break;
        };
        let notified = cancellation
            .run_until_cancelled(notify_change(&outbound_tx, tests))
            .await;
        match notified {
            None => break,
            Some(Ok(())) => {}
            Some(Err(error)) => {
                debug!("Server: stopping file-change notifications: {error}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tryke_testing::TestProject;

    use super::*;

    #[tokio::test]
    async fn watcher_stops_while_change_discovery_waits() {
        let project = TestProject::new().expect("create test project");
        let root = project.root();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let discoverer = Arc::new(Mutex::new(Discoverer::from_parts(
            root,
            src_roots,
            &[],
            None,
        )));
        let watcher = FileWatcher::spawn(root, &[]).expect("watch project");
        let (outbound_tx, _outbound_rx) = mpsc::channel(8);
        let cancellation = CancellationToken::new();

        // Holding the lock keeps change discovery waiting until shutdown.
        let guard = discoverer.lock().await;
        let task = tokio::spawn(watch_files(
            watcher,
            Arc::clone(&discoverer),
            outbound_tx.downgrade(),
            cancellation.clone(),
        ));
        std::fs::write(
            root.join("test_new.py"),
            "from tryke import test\n\n@test\ndef test_new(): pass\n",
        )
        .expect("write changed file");

        // `with_discoverer` takes its own `Arc` before waiting for the lock.
        let waiting = tokio::time::timeout(Duration::from_secs(10), async {
            while Arc::strong_count(&discoverer) < 3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        // Only the test's sender may be strong, or closing input could not
        // end the session while discovery runs.
        let senders_during_discovery = outbound_tx.strong_count();
        cancellation.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(5), task).await;
        drop(guard);

        assert!(waiting, "the change should reach discovery");
        assert_eq!(
            senders_during_discovery, 1,
            "the watcher must not hold a strong sender during discovery"
        );
        assert!(
            matches!(stopped, Ok(Ok(()))),
            "shutdown must not wait for change discovery: {stopped:?}"
        );
    }
}
