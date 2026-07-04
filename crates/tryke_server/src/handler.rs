use std::{collections::HashSet, error::Error, fmt, path::PathBuf, sync::Arc, time::Instant};

use anyhow::Context as _;
use bytes::Bytes;
use log::debug;
use serde::Serialize;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter},
    sync::{Mutex, mpsc},
};
use tokio_stream::StreamExt;
use tryke_runner::{DistMode, WorkerPool, partition_with_hooks};
use tryke_types::filter::TestFilter;
use tryke_types::{RunSummary, TestItem, TestOutcome};

use crate::protocol::{
    DidChangeParams, DiscoverCompleteParams, ErrorResponse, INVALID_PARAMS, METHOD_NOT_FOUND,
    Notification, NotificationMethod, Request, RequestMethod, Response, RunCompleteParams,
    RunParams, RunResponse, RunStartParams, TestCompleteParams,
};

/// Manages communication with a client over the given reader/writer
pub struct ConnectionHandler<R, W> {
    /// Reader to read message off of.
    reader: R,

    /// Writer to write messages back to the client.
    writer: W,

    /// Test discoverer.
    discoverer: Arc<tokio::sync::Mutex<tryke_discovery::Discoverer>>,

    /// Worker pool.
    worker_pool: Arc<WorkerPool>,

    /// Outbound message queue.
    ///
    /// Responses and asynchronous notifications share this queue.
    outbound_rx: mpsc::Receiver<Bytes>,
    outbound_tx: mpsc::Sender<Bytes>,

    /// Serializes test runs that share the worker pool.
    run_lock: Arc<Mutex<()>>,
}

impl<R, W> ConnectionHandler<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(
        reader: R,
        writer: W,
        discoverer: Arc<tokio::sync::Mutex<tryke_discovery::Discoverer>>,
        outbound_rx: mpsc::Receiver<Bytes>,
        outbound_tx: mpsc::Sender<Bytes>,
        worker_pool: Arc<WorkerPool>,
        run_lock: Arc<Mutex<()>>,
    ) -> Self {
        Self {
            reader,
            writer,
            discoverer,
            worker_pool,
            outbound_rx,
            outbound_tx,
            run_lock,
        }
    }

    /// Runs the server protocol until the client closes its input.
    ///
    /// Requests arrive as newline-delimited JSON-RPC messages. Responses and
    /// asynchronous notifications share one outbound queue and one writer task.
    ///
    /// # Errors
    /// Returns an error if reading a request, serializing a message, or
    /// writing to the client fails.
    pub async fn run(self) -> anyhow::Result<()> {
        let Self {
            reader,
            writer,
            discoverer,
            worker_pool: pool,
            outbound_rx,
            outbound_tx,
            run_lock,
        } = self;

        // The writer task is the sole owner of the client output. Routing both
        // responses and asynchronous notifications through it prevents
        // concurrent writes from interleaving JSON-RPC messages.
        let mut writer_task = tokio::spawn(async move {
            let mut writer = BufWriter::new(writer);
            let mut outbound_rx = outbound_rx;

            while let Some(bytes) = outbound_rx.recv().await {
                writer
                    .write_all(&bytes)
                    .await
                    .context("failed to write to client")?;
                writer.flush().await.context("failed to write to client")?;
            }

            Ok::<(), anyhow::Error>(())
        });

        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();

            // A connection cannot make progress once its writer stops, so wait
            // for either the next request or early writer termination.
            let read_result = tokio::select! {
                result = reader.read_line(&mut line) => result,
                result = &mut writer_task => {
                    return result.context("outbound writer task failed")?;
                }
            };

            match read_result {
                // EOF is the client's normal shutdown signal.
                Ok(0) => break,
                Ok(_) => {}
                Err(error) => {
                    // A read failure makes further output irrelevant.
                    writer_task.abort();
                    let _ = writer_task.await;
                    return Err(error.into());
                }
            }

            // Request handlers can enqueue notifications and optionally return
            // a response. The response goes through the same queue so only the
            // writer task ever touches the transport.
            let response =
                match handle_request(&line, &discoverer, &outbound_tx, &pool, &run_lock).await {
                    Ok(response) => response,
                    Err(_) if outbound_tx.is_closed() => {
                        // The writer owns the useful transport error. Await it
                        // instead of returning the secondary channel error.
                        return writer_task.await.context("outbound writer task failed")?;
                    }
                    Err(error) => {
                        // Stop the writer before returning a request-processing
                        // error so it cannot outlive the connection.
                        writer_task.abort();
                        let _ = writer_task.await;
                        return Err(error);
                    }
                };

            if let Some(bytes) = response
                && outbound_tx.send(bytes).await.is_err()
            {
                // A closed receiver means the writer has already stopped.
                return writer_task.await.context("outbound writer task failed")?;
            }
        }

        // Other server tasks retain outbound sender clones, so EOF alone does
        // not close the queue. Cancel the writer explicitly; cancellation is
        // therefore the expected successful shutdown result.
        writer_task.abort();

        match writer_task.await {
            Err(error) if error.is_cancelled() => Ok(()),
            result => result.context("outbound writer task failed")?,
        }
    }
}

#[derive(Debug)]
pub(crate) enum NotificationError {
    Serialize {
        method: NotificationMethod,
        source: serde_json::Error,
    },
    Closed {
        method: NotificationMethod,
    },
}

impl fmt::Display for NotificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize { method, source } => {
                write!(f, "failed to serialize {method} notification: {source}")
            }
            Self::Closed { method } => {
                write!(
                    f,
                    "outbound channel closed while sending {method} notification"
                )
            }
        }
    }
}

impl Error for NotificationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialize { source, .. } => Some(source),
            Self::Closed { .. } => None,
        }
    }
}

pub(crate) async fn send_notification<T: Serialize>(
    tx: &mpsc::Sender<Bytes>,
    method: NotificationMethod,
    params: T,
) -> Result<(), NotificationError> {
    let notification = Notification {
        jsonrpc: "2.0".to_string(),
        method,
        params,
    };
    let mut bytes = serde_json::to_vec(&notification)
        .map_err(|source| NotificationError::Serialize { method, source })?;
    bytes.push(b'\n');
    tx.send(Bytes::from(bytes))
        .await
        .map_err(|_| NotificationError::Closed { method })
}

pub(crate) async fn apply_change(
    discoverer: &Mutex<tryke_discovery::Discoverer>,
    outbound_tx: &mpsc::Sender<Bytes>,
    paths: &[PathBuf],
) -> Result<(), NotificationError> {
    if paths.is_empty() {
        return Ok(());
    }

    let impact = discoverer.lock().await.apply_changes(paths);
    if impact.paths.is_empty() {
        debug!("apply_change: no eligible paths");
        return Ok(());
    }

    debug!(
        "apply_change: {} affected modules, {} affected tests",
        impact.affected_modules.len(),
        impact.affected_tests.len(),
    );
    send_notification(
        outbound_tx,
        NotificationMethod::DiscoverComplete,
        DiscoverCompleteParams {
            tests: impact.affected_tests,
        },
    )
    .await
}

fn select_tests(run_params: &RunParams, all_tests: Vec<TestItem>) -> Vec<TestItem> {
    let mut tests = match &run_params.tests {
        Some(ids) => all_tests
            .into_iter()
            .filter(|test| ids.contains(&test.id()))
            .collect(),
        None => all_tests,
    };
    let paths = run_params.paths.as_deref().unwrap_or_default();
    if let Ok(filter) = TestFilter::from_args(
        paths,
        run_params.filter.as_deref(),
        run_params.markers.as_deref(),
    ) {
        tests = filter.apply(tests);
    }
    tests
}

async fn execute_run(
    run_params: RunParams,
    discoverer: &tokio::sync::Mutex<tryke_discovery::Discoverer>,
    outbound_tx: &mpsc::Sender<Bytes>,
    pool: &WorkerPool,
    run_lock: &Mutex<()>,
) -> anyhow::Result<(String, RunSummary)> {
    let run_id = run_params.run_id.clone();
    let _run_guard = run_lock.lock().await;
    pool.restart_workers().await;
    let discovery_start = Instant::now();
    let (all_tests, hooks) = {
        let guard = discoverer.lock().await;
        (guard.tests(), guard.hooks())
    };
    let tests = select_tests(&run_params, all_tests);
    let discovery_duration = discovery_start.elapsed();

    let file_count = tests
        .iter()
        .filter_map(|t| t.file_path.as_ref())
        .collect::<HashSet<_>>()
        .len();

    let start_time = chrono::Local::now().format("%H:%M:%S").to_string();

    send_notification(
        outbound_tx,
        NotificationMethod::RunStart,
        RunStartParams {
            run_id: run_id.clone(),
            tests: tests.clone(),
        },
    )
    .await?;

    let test_start = Instant::now();
    // Server uses test-level distribution by default.
    let partition = partition_with_hooks(tests, &hooks, DistMode::Test);
    for warning in &partition.warnings {
        log::warn!("{}", warning.message);
    }
    let mut stream = pool.submit(partition.units);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut xfailed = 0usize;
    let mut todo = 0usize;

    while let Some(result) = stream.next().await {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } | TestOutcome::XPassed => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
            TestOutcome::Error { .. } => errors += 1,
            TestOutcome::XFailed { .. } => xfailed += 1,
            TestOutcome::Todo { .. } => todo += 1,
        }
        send_notification(
            outbound_tx,
            NotificationMethod::TestComplete,
            TestCompleteParams {
                run_id: run_id.clone(),
                result: result.clone(),
            },
        )
        .await?;
    }

    let test_duration = test_start.elapsed();
    let summary = RunSummary {
        passed,
        failed,
        skipped,
        errors,
        xfailed,
        todo,
        duration: discovery_duration + test_duration,
        discovery_duration: Some(discovery_duration),
        test_duration: Some(test_duration),
        file_count,
        start_time: Some(start_time),
        changed_selection: None,
    };
    send_notification(
        outbound_tx,
        NotificationMethod::RunComplete,
        RunCompleteParams {
            run_id: run_id.clone(),
            summary: summary.clone(),
        },
    )
    .await?;

    Ok((run_id, summary))
}

/// Processes one newline-delimited JSON-RPC request.
///
/// # Errors
/// Returns an error if an outbound message cannot be serialized or queued.
pub async fn handle_request(
    line: &str,
    discoverer: &tokio::sync::Mutex<tryke_discovery::Discoverer>,
    outbound_tx: &mpsc::Sender<Bytes>,
    worker_pool: &WorkerPool,
    run_lock: &Mutex<()>,
) -> anyhow::Result<Option<Bytes>> {
    let Ok(req) = serde_json::from_str::<Request>(line.trim()) else {
        return Ok(None);
    };
    let id = req.id.clone().unwrap_or(Value::Null);

    let response = match &req.method {
        RequestMethod::Ping => Response::new(id, "pong").into_json_line()?,
        RequestMethod::Discover => {
            // `discover` takes no parameters — the server already knows
            // its own project root. Any `params` a client sends are
            // ignored rather than validated, so a malformed or absent
            // `params` can never drop the response and hang a client
            // that supplied an `id`.
            let tests = discoverer.lock().await.rediscover();
            send_notification(
                outbound_tx,
                NotificationMethod::DiscoverComplete,
                DiscoverCompleteParams {
                    tests: tests.clone(),
                },
            )
            .await?;
            Response::new(id, serde_json::json!({ "tests": tests })).into_json_line()?
        }
        RequestMethod::DidChange => {
            let Some(params) = req.params else {
                return Ok(Some(
                    ErrorResponse::new(
                        req.id,
                        INVALID_PARAMS,
                        "method 'did_change' requires params with paths".to_string(),
                    )
                    .into_json_line()?,
                ));
            };
            let dc = match serde_json::from_value::<DidChangeParams>(params) {
                Ok(dc) => dc,
                Err(e) => {
                    return Ok(Some(
                        ErrorResponse::new(
                            req.id,
                            INVALID_PARAMS,
                            format!("invalid params for 'did_change': {e}"),
                        )
                        .into_json_line()?,
                    ));
                }
            };
            if dc.paths.is_empty() {
                let tests = discoverer.lock().await.rediscover();
                debug!(
                    "did_change: empty paths — full rediscover, {} tests",
                    tests.len()
                );
                send_notification(
                    outbound_tx,
                    NotificationMethod::DiscoverComplete,
                    DiscoverCompleteParams { tests },
                )
                .await?;
            } else {
                apply_change(discoverer, outbound_tx, &dc.paths).await?;
            }
            Response::new(id, "ok").into_json_line()?
        }
        RequestMethod::Run => {
            let Some(params) = req.params else {
                return Ok(Some(
                    ErrorResponse::new(
                        req.id,
                        INVALID_PARAMS,
                        "method 'run' requires params with run_id".to_string(),
                    )
                    .into_json_line()?,
                ));
            };
            let run_params = match serde_json::from_value::<RunParams>(params) {
                Ok(rp) => rp,
                Err(e) => {
                    return Ok(Some(
                        ErrorResponse::new(
                            req.id,
                            INVALID_PARAMS,
                            format!("invalid params for 'run': {e}"),
                        )
                        .into_json_line()?,
                    ));
                }
            };
            let (run_id, summary) =
                execute_run(run_params, discoverer, outbound_tx, worker_pool, run_lock).await?;
            Response::new(id, RunResponse { run_id, summary }).into_json_line()?
        }
        RequestMethod::Unknown(method) => ErrorResponse::new(
            req.id,
            METHOD_NOT_FOUND,
            format!("method not found: {method}"),
        )
        .into_json_line()?,
    };
    Ok(Some(response))
}

#[cfg(test)]
mod tests {
    use std::{
        fs, io,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll},
        time::Duration,
    };

    use bytes::Bytes;
    use log::LevelFilter;
    use serde::Serializer;
    use tokio::{
        sync::{Mutex, mpsc},
        time,
    };
    use tryke_discovery::Discoverer;
    use tryke_runner::WorkerPool;
    use tryke_testing::python_bin as test_python_bin;

    use super::*;

    struct FailingParams;

    impl Serialize for FailingParams {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(serde::ser::Error::custom("intentional failure"))
        }
    }

    struct DisconnectedWriter;

    impl AsyncWrite for DisconnectedWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "client disconnected",
            )))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn make_root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        dir
    }

    async fn make_pool() -> Arc<WorkerPool> {
        Arc::new(
            WorkerPool::spawn(
                1,
                &test_python_bin(),
                std::path::Path::new("."),
                None,
                LevelFilter::Off,
                false,
            )
            .await,
        )
    }

    fn make_run_lock() -> Arc<Mutex<()>> {
        Arc::new(Mutex::new(()))
    }

    #[tokio::test]
    async fn notification_reports_serialization_errors() {
        let (tx, _rx) = mpsc::channel(1);

        let error = send_notification(&tx, NotificationMethod::TestComplete, FailingParams)
            .await
            .expect_err("serialization should fail");

        assert!(matches!(
            error,
            NotificationError::Serialize {
                method: NotificationMethod::TestComplete,
                ..
            }
        ));
        assert!(error.to_string().contains("intentional failure"));
    }

    #[tokio::test]
    async fn notification_reports_closed_channels() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);

        let error = send_notification(&tx, NotificationMethod::RunComplete, ())
            .await
            .expect_err("send should fail");

        assert!(matches!(
            error,
            NotificationError::Closed {
                method: NotificationMethod::RunComplete,
            }
        ));
    }

    #[tokio::test]
    async fn notification_waits_for_capacity_instead_of_dropping_messages() {
        let (tx, mut rx) = mpsc::channel(1);
        send_notification(&tx, NotificationMethod::DiscoverComplete, ())
            .await
            .unwrap();

        let mut second = Box::pin(send_notification(&tx, NotificationMethod::RunStart, ()));
        assert!(
            time::timeout(Duration::from_millis(10), &mut second)
                .await
                .is_err(),
            "second send should wait while the queue is full",
        );

        rx.recv().await.expect("first notification");
        time::timeout(Duration::from_secs(1), second)
            .await
            .expect("second send should resume")
            .unwrap();
        let second = rx.recv().await.expect("second notification");
        let value: serde_json::Value = serde_json::from_slice(&second).unwrap();
        assert_eq!(value["method"], "run_start");
    }

    #[tokio::test]
    async fn ping_returns_pong() {
        let dir = make_root();
        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap()
        .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "pong");
    }

    #[tokio::test]
    async fn discover_returns_tests() {
        let dir = make_root();
        fs::write(dir.path().join("test_x.py"), "@test\ndef test_x(): pass\n")
            .expect("write test file");
        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"discover","params":{"root":"/ignored"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap()
        .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert!(val["result"]["tests"].is_array());
    }

    #[tokio::test]
    async fn discover_without_params_returns_tests() {
        // Regression: `discover` must answer even when the client omits
        // `params`. It carries no parameters, so a missing `params` field
        // must not drop the response (which would hang a client that sent
        // an `id`).
        let dir = make_root();
        fs::write(dir.path().join("test_x.py"), "@test\ndef test_x(): pass\n")
            .expect("write test file");
        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"discover"}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap()
        .expect("discover without params must return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["id"], 1);
        assert!(val["result"]["tests"].is_array());
    }

    #[tokio::test]
    async fn run_enqueues_notifications() {
        let dir = make_root();
        let (tx, mut rx) = mpsc::channel(64);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"root":"/ignored","tests":null,"run_id":"r1"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap();

        let mut methods = vec![];
        while let Ok(bytes) = rx.try_recv() {
            let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if let Some(m) = val["method"].as_str() {
                methods.push(m.to_string());
            }
        }
        assert!(methods.contains(&"run_start".to_string()));
        assert!(methods.contains(&"run_complete".to_string()));
    }

    #[tokio::test]
    async fn run_without_run_id_returns_invalid_params() {
        let dir = make_root();
        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"tests":null}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap()
        .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn run_notifications_include_run_id() {
        let dir = make_root();
        let (tx, mut rx) = mpsc::channel(64);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"run_id":"test-run-xyz","tests":null}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap()
        .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"]["run_id"], "test-run-xyz");

        while let Ok(bytes) = rx.try_recv() {
            let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let method = val["method"].as_str().unwrap_or("");
            if matches!(method, "run_start" | "run_complete" | "test_complete") {
                assert_eq!(
                    val["params"]["run_id"], "test-run-xyz",
                    "notification {method} missing run_id"
                );
            }
        }
    }

    #[tokio::test]
    async fn run_uses_cached_tests_not_rediscover() {
        let dir = make_root();
        fs::write(dir.path().join("test_x.py"), "@test\ndef test_x(): pass\n")
            .expect("write initial file");
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));

        // Populate cache via discover
        let (tx, _rx) = mpsc::channel(64);
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"discover","params":{"root":"/ignored"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap();

        // Write a new file to disk without calling discover again
        fs::write(dir.path().join("test_y.py"), "@test\ndef test_y(): pass\n")
            .expect("write second file");

        // Run should return only cached tests (test_x), not pick up test_y
        let (tx2, mut rx2) = mpsc::channel(64);
        handle_request(
            r#"{"jsonrpc":"2.0","id":2,"method":"run","params":{"root":"/ignored","tests":null,"run_id":"r1"}}"#,
            &disc,
            &tx2,
            &pool,
            &run_lock,
        )
        .await
        .unwrap();

        let mut run_start_count = None;
        while let Ok(bytes) = rx2.try_recv() {
            let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if val["method"] == "run_start" {
                run_start_count = Some(val["params"]["tests"].as_array().unwrap().len());
            }
        }
        assert_eq!(
            run_start_count,
            Some(1),
            "run should use cached tests, not rediscover"
        );
    }

    #[tokio::test]
    async fn did_change_updates_discovery_and_enqueues_notification() {
        // `did_change` is the in-band signal that makes server mode
        // race-free for cooperating clients. Verify the two observable
        // side-effects:
        //   1. It returns "ok"
        //   2. It enqueues a `discover_complete` so the client refreshes
        //      its UI.
        let dir = make_root();
        let test_file = dir.path().join("test_x.py");
        fs::write(&test_file, "@test\ndef test_x(): pass\n").expect("write test file");
        let (tx, mut rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        // Populate discovery so `affected_modules` returns non-empty.
        disc.lock().await.rediscover();

        let pool = make_pool().await;
        let run_lock = make_run_lock();

        // Build the request via serde so the path is JSON-escaped
        // (Windows backslashes are escape characters in JSON strings —
        // raw `r#"..."#` interpolation produces invalid input on CI).
        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [test_file] },
        }))
        .unwrap();
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock)
            .await
            .unwrap()
            .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        let mut saw_discover_complete = false;
        while let Ok(bytes) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if v["method"] == "discover_complete" {
                saw_discover_complete = true;
            }
        }
        assert!(
            saw_discover_complete,
            "did_change must enqueue a discover_complete notification",
        );
    }

    #[tokio::test]
    async fn did_change_empty_paths_triggers_full_rediscover() {
        // Panic-button form: client doesn't know which files changed.
        // Server should re-scan everything.
        let dir = make_root();
        fs::write(dir.path().join("test_y.py"), "@test\ndef test_y(): pass\n")
            .expect("write test file");
        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();

        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"did_change","params":{"paths":[]}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap()
        .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
    }

    #[tokio::test]
    async fn did_change_without_params_returns_invalid_params() {
        let dir = make_root();
        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"did_change"}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap()
        .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn did_change_ignores_paths_outside_root() {
        // Defence-in-depth: `did_change` MUST refuse to refresh
        // discovery for files outside the configured project root —
        // otherwise a hostile client could pollute the import graph or
        // trigger arbitrary file parses.
        //
        // Setup: two tempdirs, one is the project root and contains a
        // test file, the other is "outside" and also contains a test
        // file. did_change sends both paths; only the in-root one
        // should reach discovery.
        let dir = make_root();
        let outside = tempfile::tempdir().expect("outside tempdir");
        let inside_file = dir.path().join("test_inside.py");
        let outside_file = outside.path().join("test_outside.py");
        fs::write(&inside_file, "@test\ndef test_inside(): pass\n").expect("write inside");
        fs::write(&outside_file, "@test\ndef test_outside(): pass\n").expect("write outside");

        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        disc.lock().await.rediscover();
        let pool = make_pool().await;
        let run_lock = make_run_lock();

        // Both paths in the request; only inside_file should survive.
        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [outside_file, inside_file] },
        }))
        .unwrap();
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock)
            .await
            .unwrap()
            .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
    }

    #[tokio::test]
    async fn did_change_excluded_paths_do_not_reach_discovery() {
        // The FS watcher applies `.gitignore` + `[tool.tryke] exclude`
        // before calling apply_change. Without parity in the
        // `did_change` path, a cooperating-but-misconfigured (or
        // malicious-local) client could ingest a `.venv/whatever.py`
        // into the import graph — and a subsequent `run` would try
        // to import + execute it.
        let dir = make_root();
        // Discoverer constructed with an explicit exclude (mirroring
        // `[tool.tryke] exclude = ["vendored/**"]`). The path we send
        // lives under that excluded prefix.
        let excluded_dir = dir.path().join("vendored");
        fs::create_dir(&excluded_dir).expect("mkdir");
        let excluded_file = excluded_dir.join("test_vendored.py");
        fs::write(&excluded_file, "@test\ndef test_v(): pass\n").expect("write");

        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let excludes = vec!["vendored/**".to_string()];
        let disc = Arc::new(Mutex::new(Discoverer::new(
            root, src_roots, &excludes, None,
        )));
        disc.lock().await.rediscover();
        let pool = make_pool().await;
        let run_lock = make_run_lock();

        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [excluded_file] },
        }))
        .unwrap();
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock)
            .await
            .unwrap()
            .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        assert!(disc.lock().await.tests().is_empty());
    }

    #[tokio::test]
    async fn did_change_non_py_paths_are_ignored() {
        // A `did_change` for non-Python files (README.md, pyproject.toml,
        // config dotfiles) must not enter discovery.
        let dir = make_root();
        let readme = dir.path().join("README.md");
        let pyproject = dir.path().join("pyproject.toml"); // already exists from make_root
        fs::write(&readme, "# project\n").expect("write readme");

        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        disc.lock().await.rediscover();
        let pool = make_pool().await;
        let run_lock = make_run_lock();

        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [readme, pyproject] },
        }))
        .unwrap();
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock)
            .await
            .unwrap()
            .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        assert!(disc.lock().await.tests().is_empty());
    }

    #[tokio::test]
    async fn did_change_all_paths_outside_root_is_noop() {
        // When every path is outside root, discovery must not change.
        let dir = make_root();
        let outside = tempfile::tempdir().expect("outside tempdir");
        let outside_file = outside.path().join("test_outside.py");
        fs::write(&outside_file, "@test\ndef test_outside(): pass\n").expect("write outside");

        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();

        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [outside_file] },
        }))
        .unwrap();
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock)
            .await
            .unwrap()
            .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        assert!(disc.lock().await.tests().is_empty());
    }

    #[tokio::test]
    async fn did_change_accepts_just_deleted_file_under_root() {
        // Regression: when a `did_change` arrives for a file the user
        // just deleted, `p.canonicalize()` fails (no inode to resolve).
        // The old fallback compared the literal input path against a
        // canonical root, which silently dropped legitimate deletions
        // when the workspace was under a symlinked prefix (macOS's
        // `/var → /private/var` is the common trigger). New behaviour:
        // canonicalise the parent and join the file name, so the
        // workspace prefix is normalised even when the leaf is gone.
        let dir = make_root();
        // Create then delete a file so canonicalize(file) will fail
        // but canonicalize(parent) succeeds. Discovery doesn't need
        // to know about it — we're testing the filter, not discovery.
        let test_file = dir.path().join("test_gone.py");
        fs::write(&test_file, "x = 1\n").expect("write");
        fs::remove_file(&test_file).expect("rm");

        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        disc.lock().await.rediscover();
        let pool = make_pool().await;
        let run_lock = make_run_lock();

        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [test_file] },
        }))
        .unwrap();
        // Pass the *non-canonical* dir.path() as root to simulate the
        // symlinked-prefix case — the handler will canonicalise it via
        // `filter_paths_in_root`'s caller (we pass canonicalised root
        // in production via the server). But here we want to verify the
        // filter survives a non-existent leaf, which is the bug the
        // P2 review feedback was about.
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock)
            .await
            .unwrap()
            .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(
            val["result"], "ok",
            "deleted-file did_change must not be rejected by the filter"
        );
    }

    #[tokio::test]
    async fn did_change_empty_paths_enqueues_discover_complete() {
        // P2: empty-paths form does a full rediscover; it MUST also
        // enqueue `discover_complete` so the client sees the
        // refresh. Without this, the empty-paths path leaves clients
        // permanently stale until the FS watcher fires (which may
        // never happen if changes are still pending).
        let dir = make_root();
        fs::write(dir.path().join("test_x.py"), "@test\ndef test_x(): pass\n")
            .expect("write test file");
        let (tx, mut rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();

        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"did_change","params":{"paths":[]}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap()
        .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");

        let mut saw_discover_complete = false;
        while let Ok(bytes) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if v["method"] == "discover_complete" {
                saw_discover_complete = true;
            }
        }
        assert!(
            saw_discover_complete,
            "empty-paths did_change must enqueue discover_complete",
        );
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let dir = make_root();
        let (tx, _rx) = mpsc::channel(16);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"unknown_method"}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap()
        .expect("request should return a response");
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["error"]["code"], METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn session_receives_notifications_and_response() {
        // A single session must see both the run notifications and the
        // id-bearing response on the same stream.
        let dir = make_root();
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let (outbound_tx, outbound_rx) = mpsc::channel::<Bytes>(64);
        let run_lock = make_run_lock();

        let (client, server_side) = tokio::io::duplex(1 << 16);
        let (server_r, server_w) = tokio::io::split(server_side);
        tokio::spawn(async move {
            ConnectionHandler::new(
                server_r,
                server_w,
                disc,
                outbound_rx,
                outbound_tx,
                pool,
                run_lock,
            )
            .run()
            .await
            .expect("connection handler should run");
        });

        let (client_r, mut client_w) = tokio::io::split(client);
        let run_req = r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"root":"/ignored","tests":null,"run_id":"r1"}}"#;
        client_w
            .write_all(format!("{run_req}\n").as_bytes())
            .await
            .unwrap();

        // Keep reading until the writer task has delivered both.
        let mut reader = BufReader::new(client_r);
        let mut saw_notification = false;
        let mut response: Option<serde_json::Value> = None;
        while !saw_notification || response.is_none() {
            let mut line = String::new();
            time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
                .await
                .expect("session went quiet before notification + response arrived")
                .unwrap();
            let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            if v.get("method").is_some() {
                saw_notification = true;
            } else if v.get("id").is_some() {
                response = Some(v);
            }
        }
        let response = response.expect("loop exits only once the response is seen");
        assert_eq!(response["result"]["run_id"], "r1");
    }

    #[tokio::test]
    async fn connection_handler_reports_writer_disconnect() {
        let dir = make_root();
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let discoverer = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        let pool = make_pool().await;
        let (outbound_tx, outbound_rx) = mpsc::channel(1);
        let run_lock = make_run_lock();
        let (mut client, server_reader) = tokio::io::duplex(64);

        let handler = ConnectionHandler::new(
            server_reader,
            DisconnectedWriter,
            discoverer,
            outbound_rx,
            outbound_tx,
            pool,
            run_lock,
        );
        client
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
            .await
            .unwrap();

        let error = time::timeout(Duration::from_secs(1), handler.run())
            .await
            .expect("handler should stop after writer disconnect")
            .expect_err("writer disconnect should be reported");
        assert!(
            error.to_string().contains("failed to write to client"),
            "unexpected writer error: {error:#}",
        );
    }

    #[tokio::test]
    async fn run_with_filter_restricts_tests() {
        let dir = make_root();
        fs::write(
            dir.path().join("test_x.py"),
            "@test\ndef test_alpha(): pass\n\n@test\ndef test_beta(): pass\n",
        )
        .expect("write test file");
        let (tx, mut rx) = mpsc::channel(64);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        // Populate cache
        disc.lock().await.rediscover();
        let pool = make_pool().await;
        let run_lock = make_run_lock();
        handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"filter":"alpha","run_id":"r1"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap();

        let mut run_start_count = None;
        while let Ok(bytes) = rx.try_recv() {
            let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if val["method"] == "run_start" {
                run_start_count = Some(val["params"]["tests"].as_array().unwrap().len());
            }
        }
        assert_eq!(
            run_start_count,
            Some(1),
            "filter should restrict to only test_alpha"
        );
    }

    #[tokio::test]
    async fn concurrent_runs_are_serialized() {
        // Two concurrent `run` calls on the same pool should execute
        // back-to-back, not interleaved. Proof: every test_complete
        // notification between run_start(run_id=A) and run_complete(run_id=A)
        // carries run_id=A, and same for B.
        let dir = make_root();
        fs::write(
            dir.path().join("test_x.py"),
            "@test\ndef test_alpha(): pass\n\n@test\ndef test_beta(): pass\n",
        )
        .expect("write test file");
        let (tx, mut rx) = mpsc::channel(256);
        let root = dir.path();
        let src_roots = vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())];
        let disc = Arc::new(Mutex::new(Discoverer::new(root, src_roots, &[], None)));
        disc.lock().await.rediscover();
        let pool = make_pool().await;
        let run_lock = make_run_lock();

        let disc_a = Arc::clone(&disc);
        let tx_a = tx.clone();
        let pool_a = Arc::clone(&pool);
        let run_lock_a = Arc::clone(&run_lock);
        let handle_a = tokio::spawn(async move {
            handle_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"run_id":"A","tests":null}}"#,
                &disc_a,
                &tx_a,
                &pool_a,
                &run_lock_a,
            )
            .await
            .unwrap();
        });
        let handle_b = tokio::spawn(async move {
            handle_request(
                r#"{"jsonrpc":"2.0","id":2,"method":"run","params":{"run_id":"B","tests":null}}"#,
                &disc,
                &tx,
                &pool,
                &run_lock,
            )
            .await
            .unwrap();
        });
        handle_a.await.unwrap();
        handle_b.await.unwrap();

        // Collect all notifications and walk them in order. Between
        // run_start(X) and run_complete(X), every event must have run_id=X.
        let mut events: Vec<(String, String)> = vec![];
        while let Ok(bytes) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if let Some(m) = v["method"].as_str() {
                let rid = v["params"]["run_id"].as_str().unwrap_or("").to_string();
                events.push((m.to_string(), rid));
            }
        }

        let mut active: Option<String> = None;
        for (method, rid) in &events {
            match method.as_str() {
                "run_start" => {
                    assert!(
                        active.is_none(),
                        "run_start while another run is active: events={events:?}"
                    );
                    active = Some(rid.clone());
                }
                "run_complete" => {
                    assert_eq!(
                        active.as_ref(),
                        Some(rid),
                        "run_complete for {rid} while active is {active:?}"
                    );
                    active = None;
                }
                "test_complete" => {
                    assert_eq!(
                        active.as_ref(),
                        Some(rid),
                        "test_complete for {rid} while active is {active:?}"
                    );
                }
                _ => {}
            }
        }
        assert!(active.is_none(), "run never completed: events={events:?}");
    }
}
