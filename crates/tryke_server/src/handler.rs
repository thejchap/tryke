use std::{sync::Arc, time::Duration};

use bytes::Bytes;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::TcpStream,
    sync::{Mutex, broadcast},
};
use tryke_types::{RunSummary, TestOutcome, TestResult};

use crate::protocol::{
    DiscoverCompleteParams, DiscoverParams, ErrorResponse, METHOD_NOT_FOUND, Notification, Request,
    Response, RpcError, RunCompleteParams, RunParams, RunStartParams, TestCompleteParams,
};

pub struct ConnectionHandler {
    stream: TcpStream,
    disc: Arc<std::sync::Mutex<tryke_discovery::Discoverer>>,
    broadcast_rx: broadcast::Receiver<Bytes>,
    broadcast_tx: broadcast::Sender<Bytes>,
}

impl ConnectionHandler {
    pub fn new(
        stream: TcpStream,
        disc: Arc<std::sync::Mutex<tryke_discovery::Discoverer>>,
        broadcast_rx: broadcast::Receiver<Bytes>,
        broadcast_tx: broadcast::Sender<Bytes>,
    ) -> Self {
        Self {
            stream,
            disc,
            broadcast_rx,
            broadcast_tx,
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
                    let line_owned = line.clone();
                    let response = tokio::task::spawn_blocking(move || {
                        handle_request(&line_owned, &disc, &bcast_tx)
                    })
                    .await
                    .unwrap_or(None);
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

fn fake_results(tests: &[tryke_types::TestItem]) -> Vec<TestResult> {
    tests
        .iter()
        .map(|test| TestResult {
            test: test.clone(),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(0),
            stdout: String::new(),
            stderr: String::new(),
        })
        .collect()
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

#[must_use]
#[expect(clippy::missing_panics_doc)]
pub fn handle_request(
    line: &str,
    disc: &std::sync::Mutex<tryke_discovery::Discoverer>,
    bcast_tx: &broadcast::Sender<Bytes>,
) -> Option<Vec<u8>> {
    let req: Request = serde_json::from_str(line.trim()).ok()?;
    let id = req.id.clone().unwrap_or(Value::Null);

    match req.method.as_str() {
        "ping" => serialize_response(id, "pong"),
        "discover" => {
            let _params: DiscoverParams = serde_json::from_value(req.params?).ok()?;
            let tests = disc.lock().unwrap().rediscover();
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
            let filter = req
                .params
                .and_then(|p| serde_json::from_value::<RunParams>(p).ok())
                .and_then(|p| p.tests);

            let all_tests = disc.lock().unwrap().tests();
            let tests = match &filter {
                Some(ids) => all_tests
                    .into_iter()
                    .filter(|t| ids.contains(&t.id()))
                    .collect::<Vec<_>>(),
                None => all_tests,
            };

            broadcast_notification(
                bcast_tx,
                "run_start",
                RunStartParams {
                    tests: tests.clone(),
                },
            );

            let results = fake_results(&tests);
            let mut passed = 0usize;
            let mut failed = 0usize;
            let mut skipped = 0usize;

            for result in &results {
                match &result.outcome {
                    TestOutcome::Passed => passed += 1,
                    TestOutcome::Failed { .. } => failed += 1,
                    TestOutcome::Skipped { .. } => skipped += 1,
                }
                broadcast_notification(
                    bcast_tx,
                    "test_complete",
                    TestCompleteParams {
                        result: result.clone(),
                    },
                );
            }

            let summary = RunSummary {
                passed,
                failed,
                skipped,
                duration: Duration::from_millis(0),
            };
            broadcast_notification(
                bcast_tx,
                "run_complete",
                RunCompleteParams {
                    summary: summary.clone(),
                },
            );

            serialize_response(id, serde_json::json!({ "summary": summary }))
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
    use std::{
        fs,
        sync::{Arc, Mutex},
    };

    use bytes::Bytes;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream},
        sync::broadcast,
    };
    use tryke_discovery::Discoverer;

    use super::*;

    fn make_root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        dir
    }

    #[tokio::test]
    async fn ping_returns_pong() {
        let dir = make_root();
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let resp = tokio::task::spawn_blocking(move || {
            handle_request(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, &disc, &tx)
        })
        .await
        .unwrap()
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
        let resp = tokio::task::spawn_blocking(move || {
            let line =
                r#"{"jsonrpc":"2.0","id":1,"method":"discover","params":{"root":"/ignored"}}"#;
            handle_request(line, &disc, &tx)
        })
        .await
        .unwrap()
        .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert!(val["result"]["tests"].is_array());
    }

    #[tokio::test]
    async fn run_broadcasts_notifications() {
        let dir = make_root();
        let (tx, mut rx) = broadcast::channel(64);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        tokio::task::spawn_blocking(move || {
            let line = r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"root":"/ignored","tests":null}}"#;
            handle_request(line, &disc, &tx)
        })
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
    async fn run_uses_cached_tests_not_rediscover() {
        let dir = make_root();
        fs::write(dir.path().join("test_x.py"), "@test\ndef test_x(): pass\n")
            .expect("write initial file");
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));

        // populate cache via discover
        let (tx, _rx) = broadcast::channel(64);
        let d = Arc::clone(&disc);
        tokio::task::spawn_blocking(move || {
            handle_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"discover","params":{"root":"/ignored"}}"#,
                &d,
                &tx,
            )
        })
        .await
        .unwrap();

        // write a new file to disk without calling discover again
        fs::write(dir.path().join("test_y.py"), "@test\ndef test_y(): pass\n")
            .expect("write second file");

        // run should return only cached tests (test_x), not pick up test_y
        let (tx2, mut rx2) = broadcast::channel(64);
        let d2 = Arc::clone(&disc);
        tokio::task::spawn_blocking(move || {
            handle_request(
                r#"{"jsonrpc":"2.0","id":2,"method":"run","params":{"root":"/ignored","tests":null}}"#,
                &d2,
                &tx2,
            )
        })
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
    async fn unknown_method_returns_error() {
        let dir = make_root();
        let (tx, _rx) = broadcast::channel(16);
        let disc = Arc::new(Mutex::new(Discoverer::new(dir.path())));
        let resp = tokio::task::spawn_blocking(move || {
            handle_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"unknown_method"}"#,
                &disc,
                &tx,
            )
        })
        .await
        .unwrap()
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

        let (bcast_tx, _) = broadcast::channel::<Bytes>(64);
        let bcast_tx_clone = bcast_tx.clone();
        let disc_clone = Arc::clone(&disc);

        tokio::spawn(async move {
            for _ in 0..2u8 {
                let (stream, _) = listener.accept().await.unwrap();
                let bcast_rx = bcast_tx_clone.subscribe();
                let bcast_tx_conn = bcast_tx_clone.clone();
                let d = Arc::clone(&disc_clone);
                tokio::spawn(async move {
                    ConnectionHandler::new(stream, d, bcast_rx, bcast_tx_conn)
                        .run()
                        .await;
                });
            }
        });

        let mut c1 = TcpStream::connect(addr).await.unwrap();
        let mut c2 = TcpStream::connect(addr).await.unwrap();

        let run_req =
            r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"root":"/ignored","tests":null}}"#;
        c1.write_all(format!("{run_req}\n").as_bytes())
            .await
            .unwrap();

        let mut r1 = BufReader::new(&mut c1);
        let mut r2 = BufReader::new(&mut c2);
        let mut line1 = String::new();
        let mut line2 = String::new();

        // c1 should receive the run response
        r1.read_line(&mut line1).await.unwrap();
        let v1: serde_json::Value = serde_json::from_str(line1.trim()).unwrap();
        // c2 should receive a broadcast notification
        r2.read_line(&mut line2).await.unwrap();
        let v2: serde_json::Value = serde_json::from_str(line2.trim()).unwrap();

        // c1 gets a notification or response, c2 gets a notification
        assert!(v1.get("method").is_some() || v1.get("result").is_some());
        assert!(v2.get("method").is_some());
    }
}
