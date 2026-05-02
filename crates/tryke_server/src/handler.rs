use std::{collections::HashSet, sync::Arc, time::Instant};

use bytes::Bytes;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::TcpStream,
    sync::{Mutex, broadcast},
};
use tokio_stream::StreamExt;
use tryke_runner::{DistMode, WorkerPool, partition_with_hooks};
use tryke_types::filter::TestFilter;
use tryke_types::{RunSummary, TestOutcome};

use crate::protocol::{
    DiscoverCompleteParams, DiscoverParams, ErrorResponse, INVALID_PARAMS, METHOD_NOT_FOUND,
    Notification, Request, Response, RpcError, RunCompleteParams, RunParams, RunResponse,
    RunStartParams, TestCompleteParams,
};

pub struct ConnectionHandler {
    stream: TcpStream,
    disc: Arc<tokio::sync::Mutex<tryke_discovery::Discoverer>>,
    broadcast_rx: broadcast::Receiver<Bytes>,
    broadcast_tx: broadcast::Sender<Bytes>,
    pool: Arc<WorkerPool>,
    run_lock: Arc<Mutex<()>>,
}

impl ConnectionHandler {
    pub fn new(
        stream: TcpStream,
        disc: Arc<tokio::sync::Mutex<tryke_discovery::Discoverer>>,
        broadcast_rx: broadcast::Receiver<Bytes>,
        broadcast_tx: broadcast::Sender<Bytes>,
        pool: Arc<WorkerPool>,
        run_lock: Arc<Mutex<()>>,
    ) -> Self {
        Self {
            stream,
            disc,
            broadcast_rx,
            broadcast_tx,
            pool,
            run_lock,
        }
    }

    pub async fn run(self) {
        let (read_half, write_half) = self.stream.into_split();
        let writer = Arc::new(Mutex::new(BufWriter::new(write_half)));

        let writer_for_notif = Arc::clone(&writer);
        let mut bcast_rx = self.broadcast_rx;
        tokio::spawn(async move {
            loop {
                match bcast_rx.recv().await {
                    Ok(bytes) => {
                        let mut w = writer_for_notif.lock().await;
                        if w.write_all(&bytes).await.is_err() || w.flush().await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
        });

        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let disc = Arc::clone(&self.disc);
                    let bcast_tx = self.broadcast_tx.clone();
                    let pool = Arc::clone(&self.pool);
                    let run_lock = Arc::clone(&self.run_lock);
                    let line_owned = line.clone();
                    let response =
                        handle_request(&line_owned, &disc, &bcast_tx, &pool, &run_lock).await;
                    if let Some(bytes) = response {
                        let mut w = writer.lock().await;
                        if w.write_all(&bytes).await.is_err() || w.flush().await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn broadcast_notification<T: serde::Serialize>(
    tx: &broadcast::Sender<Bytes>,
    method: &str,
    params: T,
) {
    let notif = Notification {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        params,
    };
    if let Ok(mut bytes) = serde_json::to_vec(&notif) {
        bytes.push(b'\n');
        let _ = tx.send(Bytes::from(bytes));
    }
}

fn serialize_response<T: serde::Serialize>(id: Value, result: T) -> Option<Vec<u8>> {
    let resp = Response {
        jsonrpc: "2.0".to_string(),
        id,
        result,
    };
    serde_json::to_vec(&resp).ok().map(|mut v| {
        v.push(b'\n');
        v
    })
}

fn serialize_error(id: Option<Value>, code: i32, message: String) -> Option<Vec<u8>> {
    let resp = ErrorResponse {
        jsonrpc: "2.0".to_string(),
        id,
        error: RpcError { code, message },
    };
    serde_json::to_vec(&resp).ok().map(|mut v| {
        v.push(b'\n');
        v
    })
}

async fn execute_run(
    rp: RunParams,
    disc: &tokio::sync::Mutex<tryke_discovery::Discoverer>,
    bcast_tx: &broadcast::Sender<Bytes>,
    pool: &WorkerPool,
    run_lock: &Mutex<()>,
) -> (String, RunSummary) {
    let run_id = rp.run_id.clone();
    // Serialize concurrent runs: only one run at a time may dispatch units
    // onto the shared pool, so per-module fixture state (and test_complete
    // notification ordering) is not interleaved across clients.
    let _run_guard = run_lock.lock().await;

    let discovery_start = Instant::now();
    let guard = disc.lock().await;
    let all_tests = guard.tests();
    let hooks = guard.hooks();
    drop(guard);
    let mut tests = match &rp.tests {
        Some(ids) => all_tests
            .into_iter()
            .filter(|t| ids.contains(&t.id()))
            .collect::<Vec<_>>(),
        None => all_tests,
    };

    let paths = rp.paths.unwrap_or_default();
    if let Ok(tf) = TestFilter::from_args(&paths, rp.filter.as_deref(), rp.markers.as_deref()) {
        tests = tf.apply(tests);
    }
    let discovery_duration = discovery_start.elapsed();

    let file_count = tests
        .iter()
        .filter_map(|t| t.file_path.as_ref())
        .collect::<HashSet<_>>()
        .len();

    let start_time = chrono::Local::now().format("%H:%M:%S").to_string();

    broadcast_notification(
        bcast_tx,
        "run_start",
        RunStartParams {
            run_id: run_id.clone(),
            tests: tests.clone(),
        },
    );

    let test_start = Instant::now();
    // Server uses test-level distribution by default.
    let partition = partition_with_hooks(tests, &hooks, DistMode::Test);
    for w in &partition.warnings {
        log::warn!("{w}");
    }
    let mut stream = pool.run(partition.units);
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
        broadcast_notification(
            bcast_tx,
            "test_complete",
            TestCompleteParams {
                run_id: run_id.clone(),
                result: result.clone(),
            },
        );
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
    broadcast_notification(
        bcast_tx,
        "run_complete",
        RunCompleteParams {
            run_id: run_id.clone(),
            summary: summary.clone(),
        },
    );

    (run_id, summary)
}

pub async fn handle_request(
    line: &str,
    disc: &tokio::sync::Mutex<tryke_discovery::Discoverer>,
    bcast_tx: &broadcast::Sender<Bytes>,
    pool: &WorkerPool,
    run_lock: &Mutex<()>,
) -> Option<Vec<u8>> {
    let req: Request = serde_json::from_str(line.trim()).ok()?;
    let id = req.id.clone().unwrap_or(Value::Null);

    match req.method.as_str() {
        "ping" => serialize_response(id, "pong"),
        "discover" => {
            let _params: DiscoverParams = serde_json::from_value(req.params?).ok()?;
            let tests = disc.lock().await.rediscover();
            broadcast_notification(
                bcast_tx,
                "discover_complete",
                DiscoverCompleteParams {
                    tests: tests.clone(),
                },
            );
            serialize_response(id, serde_json::json!({ "tests": tests }))
        }
        "run" => {
            let Some(params) = req.params else {
                return serialize_error(
                    req.id,
                    INVALID_PARAMS,
                    "method 'run' requires params with run_id".to_string(),
                );
            };
            let rp = match serde_json::from_value::<RunParams>(params) {
                Ok(rp) => rp,
                Err(e) => {
                    return serialize_error(
                        req.id,
                        INVALID_PARAMS,
                        format!("invalid params for 'run': {e}"),
                    );
                }
            };
            let (run_id, summary) = execute_run(rp, disc, bcast_tx, pool, run_lock).await;
            serialize_response(id, RunResponse { run_id, summary })
        }
        _ => serialize_error(
            req.id,
            METHOD_NOT_FOUND,
            format!("method not found: {}", req.method),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use bytes::Bytes;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream},
        sync::{Mutex, broadcast},
    };
    use tryke_discovery::Discoverer;
    use tryke_runner::WorkerPool;

    use super::*;

    fn test_python_bin() -> String {
        let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (venv, fallback) = if cfg!(windows) {
            (workspace.join(".venv/Scripts/python.exe"), "python3")
        } else {
            (workspace.join(".venv/bin/python3"), "python3")
        };
        if venv.exists() {
            venv.to_string_lossy().into_owned()
        } else {
            fallback.to_owned()
        }
    }

    fn make_root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        dir
    }

    fn make_pool() -> Arc<WorkerPool> {
        Arc::new(WorkerPool::new(
            1,
            &test_python_bin(),
            std::path::Path::new("."),
        ))
    }

    fn make_run_lock() -> Arc<Mutex<()>> {
        Arc::new(Mutex::new(()))
    }

    #[tokio::test]
    async fn ping_returns_pong() {
        let dir = make_root();
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "pong");
    }

    #[tokio::test]
    async fn discover_returns_tests() {
        let dir = make_root();
        fs::write(dir.path().join("test_x.py"), "@test\ndef test_x(): pass\n")
            .expect("write test file");
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"discover","params":{"root":"/ignored"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert!(val["result"]["tests"].is_array());
    }

    #[tokio::test]
    async fn run_broadcasts_notifications() {
        let dir = make_root();
        let (tx, mut rx) = broadcast::channel(64);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"root":"/ignored","tests":null,"run_id":"r1"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await;

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
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"tests":null}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn run_broadcasts_include_run_id() {
        let dir = make_root();
        let (tx, mut rx) = broadcast::channel(64);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"run_id":"test-run-xyz","tests":null}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap();
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
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));

        // Populate cache via discover
        let (tx, _rx) = broadcast::channel(64);
        let pool = make_pool();
        let run_lock = make_run_lock();
        handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"discover","params":{"root":"/ignored"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await;

        // Write a new file to disk without calling discover again
        fs::write(dir.path().join("test_y.py"), "@test\ndef test_y(): pass\n")
            .expect("write second file");

        // Run should return only cached tests (test_x), not pick up test_y
        let (tx2, mut rx2) = broadcast::channel(64);
        handle_request(
            r#"{"jsonrpc":"2.0","id":2,"method":"run","params":{"root":"/ignored","tests":null,"run_id":"r1"}}"#,
            &disc,
            &tx2,
            &pool,
            &run_lock,
        )
        .await;

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
    async fn unknown_method_returns_error() {
        let dir = make_root();
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"unknown_method"}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await
        .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["error"]["code"], METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn two_clients_both_receive_broadcast() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = make_root();
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = Arc::new(WorkerPool::new(1, "python3", std::path::Path::new(".")));

        let (bcast_tx, _) = broadcast::channel::<Bytes>(64);
        let bcast_tx_clone = bcast_tx.clone();
        let disc_clone = Arc::clone(&disc);
        let pool_clone = Arc::clone(&pool);

        let run_lock = make_run_lock();
        let run_lock_clone = Arc::clone(&run_lock);
        tokio::spawn(async move {
            for _ in 0..2u8 {
                let (stream, _) = listener.accept().await.unwrap();
                let bcast_rx = bcast_tx_clone.subscribe();
                let bcast_tx_conn = bcast_tx_clone.clone();
                let d = Arc::clone(&disc_clone);
                let p = Arc::clone(&pool_clone);
                let rl = Arc::clone(&run_lock_clone);
                tokio::spawn(async move {
                    ConnectionHandler::new(stream, d, bcast_rx, bcast_tx_conn, p, rl)
                        .run()
                        .await;
                });
            }
        });

        let mut c1 = TcpStream::connect(addr).await.unwrap();
        let mut c2 = TcpStream::connect(addr).await.unwrap();

        let run_req = r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"root":"/ignored","tests":null,"run_id":"r1"}}"#;
        c1.write_all(format!("{run_req}\n").as_bytes())
            .await
            .unwrap();

        let mut r1 = BufReader::new(&mut c1);
        let mut r2 = BufReader::new(&mut c2);
        let mut line1 = String::new();
        let mut line2 = String::new();

        // C1 should receive the run response
        r1.read_line(&mut line1).await.unwrap();
        let v1: serde_json::Value = serde_json::from_str(line1.trim()).unwrap();
        // C2 should receive a broadcast notification
        r2.read_line(&mut line2).await.unwrap();
        let v2: serde_json::Value = serde_json::from_str(line2.trim()).unwrap();

        // C1 gets a notification or response, c2 gets a notification
        assert!(v1.get("method").is_some() || v1.get("result").is_some());
        assert!(v2.get("method").is_some());
    }

    #[tokio::test]
    async fn run_with_filter_restricts_tests() {
        let dir = make_root();
        fs::write(
            dir.path().join("test_x.py"),
            "@test\ndef test_alpha(): pass\n\n@test\ndef test_beta(): pass\n",
        )
        .expect("write test file");
        let (tx, mut rx) = broadcast::channel(64);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        // Populate cache
        disc.lock().await.rediscover();
        let pool = make_pool();
        let run_lock = make_run_lock();
        handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"filter":"alpha","run_id":"r1"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
        )
        .await;

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
        let (tx, mut rx) = broadcast::channel(256);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        disc.lock().await.rediscover();
        let pool = make_pool();
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
            .await;
        });
        let handle_b = tokio::spawn(async move {
            handle_request(
                r#"{"jsonrpc":"2.0","id":2,"method":"run","params":{"run_id":"B","tests":null}}"#,
                &disc,
                &tx,
                &pool,
                &run_lock,
            )
            .await;
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
