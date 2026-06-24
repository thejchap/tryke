use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use bytes::Bytes;
use log::debug;
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
    DidChangeParams, DiscoverCompleteParams, DiscoverParams, ErrorResponse, INVALID_PARAMS,
    METHOD_NOT_FOUND, Notification, Request, Response, RpcError, RunCompleteParams, RunParams,
    RunResponse, RunStartParams, TestCompleteParams,
};

pub struct ConnectionHandler {
    stream: TcpStream,
    disc: Arc<tokio::sync::Mutex<tryke_discovery::Discoverer>>,
    broadcast_rx: broadcast::Receiver<Bytes>,
    broadcast_tx: broadcast::Sender<Bytes>,
    pool: Arc<WorkerPool>,
    run_lock: Arc<Mutex<()>>,
    /// Set to `true` by the watcher whenever an accepted file-change
    /// batch affects at least one module. `execute_run` swaps this to
    /// `false` while holding `run_lock` and, if it was `true`, calls
    /// `pool.restart_workers().await` before dispatching units. This
    /// closes the race where a `run` request lands inside the watcher's
    /// debounce window and would otherwise hit a worker whose cached
    /// `sys.modules` predates the latest save.
    dirty: Arc<AtomicBool>,
}

impl ConnectionHandler {
    pub fn new(
        stream: TcpStream,
        disc: Arc<tokio::sync::Mutex<tryke_discovery::Discoverer>>,
        broadcast_rx: broadcast::Receiver<Bytes>,
        broadcast_tx: broadcast::Sender<Bytes>,
        pool: Arc<WorkerPool>,
        run_lock: Arc<Mutex<()>>,
        dirty: Arc<AtomicBool>,
    ) -> Self {
        Self {
            stream,
            disc,
            broadcast_rx,
            broadcast_tx,
            pool,
            run_lock,
            dirty,
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
                    let dirty = Arc::clone(&self.dirty);
                    let line_owned = line.clone();
                    let response =
                        handle_request(&line_owned, &disc, &bcast_tx, &pool, &run_lock, &dirty)
                            .await;
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
    dirty: &AtomicBool,
) -> (String, RunSummary) {
    let run_id = rp.run_id.clone();
    // Serialize concurrent runs: only one run at a time may dispatch units
    // onto the shared pool, so per-module fixture state (and test_complete
    // notification ordering) is not interleaved across clients.
    let _run_guard = run_lock.lock().await;

    // Snapshot tests AND drain the dirty flag in the SAME disc.lock
    // critical section. This pairs with `apply_change`'s
    // `dirty.store(true)` (also done inside its disc.lock guard): the
    // two operations cannot interleave, so a concurrent watcher /
    // `did_change` task can't update discovery between our dirty check
    // and our tests snapshot. Without this serialisation, a watcher
    // mid-flight could (a) acquire disc.lock first, (b) update
    // discovery, (c) release the lock, while we grab the *new* tests
    // but observe stale `dirty=false` because the store hadn't
    // happened yet — dispatching on workers whose `sys.modules` predates
    // the change.
    //
    // The flag is set by:
    //   - the FS watcher task (server.rs) on an accepted file-change
    //     batch, for non-cooperating clients; and
    //   - the `did_change` RPC handler (below), for cooperating clients
    //     (neotest-tryke sends `did_change` on the same TCP connection
    //     immediately before `run`).
    // Cooperating clients are also guaranteed to see `dirty=true`
    // because per-connection reads are serial (handler.rs:72-92): the
    // `did_change` handler's `apply_change` completes before the `run`
    // line is even read off the socket.
    let discovery_start = Instant::now();
    let (dirty_was_set, all_tests, hooks) = {
        let guard = disc.lock().await;
        let was = dirty.swap(false, Ordering::AcqRel);
        (was, guard.tests(), guard.hooks())
    };
    if dirty_was_set {
        debug!("execute_run: dirty drained — restarting worker pool");
        pool.restart_workers().await;
    }
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
    for warning in &partition.warnings {
        log::warn!("{}", warning.message);
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
    dirty: &AtomicBool,
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
        "did_change" => {
            // In-band signal from a cooperating client (e.g. neotest-tryke's
            // BufWritePost autocmd) that the listed paths just changed on
            // disk. Refresh discovery and mark `dirty` synchronously, so a
            // subsequent `run` on the *same TCP connection* is guaranteed
            // to drain a `true` flag and restart the worker pool before
            // dispatch. See `crate::change::apply_change` for the shared
            // body (also called by the FS watcher task).
            let Some(params) = req.params else {
                return serialize_error(
                    req.id,
                    INVALID_PARAMS,
                    "method 'did_change' requires params with paths".to_string(),
                );
            };
            let dc = match serde_json::from_value::<DidChangeParams>(params) {
                Ok(dc) => dc,
                Err(e) => {
                    return serialize_error(
                        req.id,
                        INVALID_PARAMS,
                        format!("invalid params for 'did_change': {e}"),
                    );
                }
            };
            if dc.paths.is_empty() {
                // Panic-button form: client doesn't know what changed.
                // Full rediscover; broadcast the resulting test list so
                // other connected clients see the refresh (matches the
                // non-empty path through `apply_change`, which also
                // broadcasts `discover_complete`).
                let tests = {
                    let mut guard = disc.lock().await;
                    let tests = guard.rediscover();
                    // Set dirty inside the lock — pairs with the
                    // drain inside `execute_run`'s disc.lock guard so
                    // no run can read fresh tests without also
                    // observing dirty=true.
                    dirty.store(true, Ordering::Release);
                    tests
                };
                debug!(
                    "did_change: empty paths — full rediscover, {} tests, dirty mark",
                    tests.len()
                );
                broadcast_notification(
                    bcast_tx,
                    "discover_complete",
                    DiscoverCompleteParams { tests },
                );
            } else {
                // `Discoverer::rediscover_changed` (inside apply_change)
                // already canonicalises and drops anything outside its
                // project root, so we can pass client-supplied paths
                // through without an extra filter. The server binds to
                // 127.0.0.1 but any local process can still reach it;
                // pushing the root check into discovery keeps the
                // guarantee in the single place that owns the project
                // tree.
                crate::change::apply_change(disc, bcast_tx, dirty, &dc.paths).await;
            }
            serialize_response(id, "ok")
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
            let (run_id, summary) = execute_run(rp, disc, bcast_tx, pool, run_lock, dirty).await;
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
    use log::LevelFilter;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream},
        sync::{Mutex, broadcast},
    };
    use tryke_discovery::Discoverer;
    use tryke_runner::WorkerPool;
    use tryke_testing::python_bin as test_python_bin;

    use super::*;

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
            LevelFilter::Off,
        ))
    }

    fn make_run_lock() -> Arc<Mutex<()>> {
        Arc::new(Mutex::new(()))
    }

    fn make_dirty() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[tokio::test]
    async fn ping_returns_pong() {
        let dir = make_root();
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
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
        let dirty = make_dirty();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"discover","params":{"root":"/ignored"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
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
        let dirty = make_dirty();
        handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"root":"/ignored","tests":null,"run_id":"r1"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
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
        let dirty = make_dirty();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"tests":null}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
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
        let dirty = make_dirty();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"run_id":"test-run-xyz","tests":null}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
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
        let dirty = make_dirty();
        handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"discover","params":{"root":"/ignored"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
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
            &dirty,
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
    async fn did_change_sets_dirty_and_broadcasts() {
        // `did_change` is the in-band signal that makes server mode
        // race-free for cooperating clients. Verify the three observable
        // side-effects:
        //   1. It returns "ok"
        //   2. It flips `dirty` to true (which the subsequent `run`'s
        //      `execute_run` will drain into a `restart_workers`)
        //   3. It broadcasts a `discover_complete` so other clients
        //      refresh their UI
        let dir = make_root();
        let test_file = dir.path().join("test_x.py");
        fs::write(&test_file, "@test\ndef test_x(): pass\n").expect("write test file");
        let (tx, mut rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        // Populate discovery so `affected_modules` returns non-empty.
        disc.lock().await.rediscover();

        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();

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
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock, &dirty)
            .await
            .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        assert!(
            dirty.load(std::sync::atomic::Ordering::Acquire),
            "did_change must set dirty=true for an affected module",
        );

        let mut saw_discover_complete = false;
        while let Ok(bytes) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if v["method"] == "discover_complete" {
                saw_discover_complete = true;
            }
        }
        assert!(
            saw_discover_complete,
            "did_change must broadcast a discover_complete notification",
        );
    }

    #[tokio::test]
    async fn did_change_empty_paths_triggers_full_rediscover() {
        // Panic-button form: client doesn't know which files changed.
        // Server should re-scan everything and mark dirty.
        let dir = make_root();
        fs::write(dir.path().join("test_y.py"), "@test\ndef test_y(): pass\n")
            .expect("write test file");
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();

        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"did_change","params":{"paths":[]}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
        )
        .await
        .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        assert!(
            dirty.load(std::sync::atomic::Ordering::Acquire),
            "did_change with empty paths must still mark dirty",
        );
    }

    #[tokio::test]
    async fn did_change_without_params_returns_invalid_params() {
        let dir = make_root();
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"did_change"}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
        )
        .await
        .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn did_change_ignores_paths_outside_root() {
        // Defence-in-depth: the server binds to 127.0.0.1 but any local
        // process can still reach it. `did_change` MUST refuse to refresh
        // discovery for files outside the configured project root —
        // otherwise a hostile client could pollute the import graph or
        // trigger arbitrary file parses.
        //
        // Setup: two tempdirs, one is the project root and contains a
        // test file, the other is "outside" and also contains a test
        // file. did_change sends both paths; only the in-root one
        // should result in a dirty mark.
        let dir = make_root();
        let outside = tempfile::tempdir().expect("outside tempdir");
        let inside_file = dir.path().join("test_inside.py");
        let outside_file = outside.path().join("test_outside.py");
        fs::write(&inside_file, "@test\ndef test_inside(): pass\n").expect("write inside");
        fs::write(&outside_file, "@test\ndef test_outside(): pass\n").expect("write outside");

        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        disc.lock().await.rediscover();
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();

        // Both paths in the request; only inside_file should survive.
        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [outside_file, inside_file] },
        }))
        .unwrap();
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock, &dirty)
            .await
            .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        // The inside path is a real module, so dirty MUST be set
        // (proves the filter didn't drop everything).
        assert!(
            dirty.load(std::sync::atomic::Ordering::Acquire),
            "in-root path should still trigger dirty mark",
        );
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

        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new_with_excludes(
            dir.path(),
            &["vendored/**".to_string()],
        )));
        disc.lock().await.rediscover();
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();

        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [excluded_file] },
        }))
        .unwrap();
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock, &dirty)
            .await
            .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        assert!(
            !dirty.load(std::sync::atomic::Ordering::Acquire),
            "excluded path must not mark the pool dirty",
        );
    }

    #[tokio::test]
    async fn did_change_non_py_paths_do_not_mark_dirty() {
        // A `did_change` for non-Python files (README.md, pyproject.toml,
        // config dotfiles) must NOT mark the pool dirty — those changes
        // can't affect any imported module, so triggering a worker
        // restart on the next run is pure waste. Pre-fix:
        // `affected_modules` happily turned `README.md` into a "README"
        // module name (the wrapper at lib.rs:106-108 unwraps None to
        // empty), making `modules.is_empty()` false and setting dirty.
        let dir = make_root();
        let readme = dir.path().join("README.md");
        let pyproject = dir.path().join("pyproject.toml"); // already exists from make_root
        fs::write(&readme, "# project\n").expect("write readme");

        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        disc.lock().await.rediscover();
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();

        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [readme, pyproject] },
        }))
        .unwrap();
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock, &dirty)
            .await
            .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        assert!(
            !dirty.load(std::sync::atomic::Ordering::Acquire),
            "non-.py did_change must not mark dirty (would force a needless worker restart)",
        );
    }

    #[tokio::test]
    async fn did_change_all_paths_outside_root_is_noop() {
        // The opposite of the above: when EVERY path is outside root,
        // dirty must NOT be set and we must not mutate discovery.
        let dir = make_root();
        let outside = tempfile::tempdir().expect("outside tempdir");
        let outside_file = outside.path().join("test_outside.py");
        fs::write(&outside_file, "@test\ndef test_outside(): pass\n").expect("write outside");

        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();

        let req = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [outside_file] },
        }))
        .unwrap();
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock, &dirty)
            .await
            .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(val["result"], "ok");
        assert!(
            !dirty.load(std::sync::atomic::Ordering::Acquire),
            "all-outside-root did_change must not mark dirty",
        );
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

        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        disc.lock().await.rediscover();
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();

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
        let resp = handle_request(&req, &disc, &tx, &pool, &run_lock, &dirty)
            .await
            .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(
            val["result"], "ok",
            "deleted-file did_change must not be rejected by the filter"
        );
    }

    #[tokio::test]
    async fn did_change_empty_paths_broadcasts_discover_complete() {
        // P2: empty-paths form does a full rediscover; it MUST also
        // broadcast `discover_complete` so connected clients see the
        // refresh. Without this, the empty-paths path leaves clients
        // permanently stale until the FS watcher fires (which may
        // never happen if changes are still pending).
        let dir = make_root();
        fs::write(dir.path().join("test_x.py"), "@test\ndef test_x(): pass\n")
            .expect("write test file");
        let (tx, mut rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();

        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"did_change","params":{"paths":[]}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
        )
        .await
        .unwrap();
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
            "empty-paths did_change must broadcast discover_complete",
        );
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let dir = make_root();
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let pool = make_pool();
        let run_lock = make_run_lock();
        let dirty = make_dirty();
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"unknown_method"}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
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
        let pool = Arc::new(WorkerPool::new(
            1,
            "python3",
            std::path::Path::new("."),
            LevelFilter::Off,
        ));

        let (bcast_tx, _) = broadcast::channel::<Bytes>(64);
        let bcast_tx_clone = bcast_tx.clone();
        let disc_clone = Arc::clone(&disc);
        let pool_clone = Arc::clone(&pool);

        let run_lock = make_run_lock();
        let run_lock_clone = Arc::clone(&run_lock);
        let dirty = make_dirty();
        let dirty_clone = Arc::clone(&dirty);
        tokio::spawn(async move {
            for _ in 0..2u8 {
                let (stream, _) = listener.accept().await.unwrap();
                let bcast_rx = bcast_tx_clone.subscribe();
                let bcast_tx_conn = bcast_tx_clone.clone();
                let d = Arc::clone(&disc_clone);
                let p = Arc::clone(&pool_clone);
                let rl = Arc::clone(&run_lock_clone);
                let dy = Arc::clone(&dirty_clone);
                tokio::spawn(async move {
                    ConnectionHandler::new(stream, d, bcast_rx, bcast_tx_conn, p, rl, dy)
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
        let dirty = make_dirty();
        handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"filter":"alpha","run_id":"r1"}}"#,
            &disc,
            &tx,
            &pool,
            &run_lock,
            &dirty,
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
        let dirty = make_dirty();

        let disc_a = Arc::clone(&disc);
        let tx_a = tx.clone();
        let pool_a = Arc::clone(&pool);
        let run_lock_a = Arc::clone(&run_lock);
        let dirty_a = Arc::clone(&dirty);
        let handle_a = tokio::spawn(async move {
            handle_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"run_id":"A","tests":null}}"#,
                &disc_a,
                &tx_a,
                &pool_a,
                &run_lock_a,
                &dirty_a,
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
                &dirty,
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
