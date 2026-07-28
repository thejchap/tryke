use std::sync::Arc;

use anyhow::Context as _;
use log::debug;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tryke_discovery::Discoverer;
use tryke_runner::WorkerPool;
use tryke_watcher::FileWatcher;

use crate::handler::{ConnectionHandler, RunDispatcher, apply_change};

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

    /// Runs the server until the client closes its input or cancellation is
    /// requested.
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
        discoverer.lock().await.rediscover();

        let watcher = match FileWatcher::spawn(&root, &excludes) {
            Ok(watcher) => watcher,
            Err(error) => {
                return match worker_pool.shutdown().await {
                    Ok(()) => Err(error),
                    Err(shutdown_error) => Err(error.context(format!(
                        "Worker pool shutdown also failed: {shutdown_error:#}"
                    ))),
                };
            }
        };
        let watcher_task = tokio::spawn(watch_files(
            watcher,
            Arc::clone(&discoverer),
            outbound_tx.clone(),
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

        lifecycle.cancel();
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
        let result = match (result, secondary_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(error)) | (Err(error), Ok(())) => Err(error),
            (Err(error), Err(secondary_error)) => {
                Err(error.context(format!("{secondary_name} also failed: {secondary_error:#}")))
            }
        };

        let watcher_result = watcher_task.await.context("File watcher task failed");

        match (result, watcher_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(error)) | (Err(error), Ok(())) => Err(error),
            (Err(error), Err(watcher_error)) => {
                Err(error.context(format!("File watcher also failed: {watcher_error:#}")))
            }
        }
    }
}

async fn watch_files(
    mut watcher: FileWatcher,
    discoverer: Arc<Mutex<Discoverer>>,
    outbound_tx: mpsc::Sender<bytes::Bytes>,
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
        if let Err(error) = apply_change(&discoverer, &outbound_tx, &batch.paths).await {
            debug!("Server: stopping file-change notifications: {error}");
            break;
        }
    }
}
