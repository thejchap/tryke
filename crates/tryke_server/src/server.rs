use std::sync::Arc;

use log::debug;
use tokio::{
    io::{AsyncRead, AsyncWrite, Stdin, Stdout},
    sync::{Mutex, mpsc},
};
use tryke_discovery::Discoverer;
use tryke_runner::WorkerPool;
#[cfg(test)]
use tryke_watcher::FileChangeBatch;
use tryke_watcher::FileWatcher;

use crate::handler::{ConnectionHandler, apply_change};

enum WatchMode {
    Filesystem,
    #[cfg(test)]
    Disabled,
    #[cfg(test)]
    Manual(mpsc::UnboundedReceiver<FileChangeBatch>),
}

pub struct Server<R, W> {
    reader: R,
    writer: W,
    worker_pool: WorkerPool,
    discoverer: Discoverer,
    watch_mode: WatchMode,
}

impl Server<Stdin, Stdout> {
    #[must_use]
    pub fn new(worker_pool: WorkerPool, discoverer: Discoverer) -> Self {
        Self::with_transport(
            worker_pool,
            discoverer,
            tokio::io::stdin(),
            tokio::io::stdout(),
        )
    }
}

impl<R, W> Server<R, W> {
    fn with_transport(
        worker_pool: WorkerPool,
        discoverer: Discoverer,
        reader: R,
        writer: W,
    ) -> Self {
        Self {
            reader,
            writer,
            worker_pool,
            discoverer,
            watch_mode: WatchMode::Filesystem,
        }
    }

    #[cfg(test)]
    fn without_file_watcher(mut self) -> Self {
        self.watch_mode = WatchMode::Disabled;
        self
    }

    #[cfg(test)]
    fn with_manual_file_watcher(
        mut self,
        changes: mpsc::UnboundedReceiver<FileChangeBatch>,
    ) -> Self {
        self.watch_mode = WatchMode::Manual(changes);
        self
    }
}

impl<R, W> Server<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    /// Runs the server over its configured reader and writer.
    ///
    /// `Server::new` configures process stdin/stdout for the normal
    /// editor-child-process protocol. Closing the reader shuts the server down.
    ///
    /// # Errors
    /// Returns an error if file watching cannot be initialized or the
    /// client transport fails.
    pub async fn serve(self) -> anyhow::Result<()> {
        let Self {
            reader,
            writer,
            worker_pool,
            discoverer,
            watch_mode,
        } = self;

        let root = discoverer.root().to_path_buf();
        let excludes = discoverer.excludes().to_vec();
        let worker_pool = Arc::new(worker_pool);
        let run_lock = Arc::new(Mutex::new(()));

        // Everything sent to the client goes through this queue
        let (outbound_tx, outbound_rx) = mpsc::channel(256);

        // Initialize the discoverer and do its initial discovery/populate import graph/test cache
        let discoverer = Arc::new(Mutex::new(discoverer));
        discoverer.lock().await.rediscover();

        let disc_for_watcher = Arc::clone(&discoverer);
        let outbound_for_watcher = outbound_tx.clone();
        let watcher_task = match watch_mode {
            WatchMode::Filesystem => {
                let mut watcher = FileWatcher::spawn(&root, &excludes)?;
                Some(tokio::spawn(async move {
                    loop {
                        let batch = match watcher.next_batch().await {
                            Ok(Some(batch)) => batch,
                            Ok(None) => break,
                            Err(error) => {
                                debug!("server: stopping file watcher: {error}");
                                break;
                            }
                        };
                        if let Err(error) =
                            apply_change(&disc_for_watcher, &outbound_for_watcher, &batch.paths)
                                .await
                        {
                            debug!("server: stopping file-change notifications: {error}");
                            break;
                        }
                    }
                }))
            }
            #[cfg(test)]
            WatchMode::Disabled => None,
            #[cfg(test)]
            WatchMode::Manual(mut changes) => Some(tokio::spawn(async move {
                loop {
                    let Some(batch) = changes.recv().await else {
                        break;
                    };
                    if let Err(error) =
                        apply_change(&disc_for_watcher, &outbound_for_watcher, &batch.paths).await
                    {
                        debug!("server: stopping file-change notifications: {error}");
                        break;
                    }
                }
            })),
        };

        debug!("server: session started");

        // Initialize and run the connection handler
        let handler = ConnectionHandler::new(
            reader,
            writer,
            Arc::clone(&discoverer),
            outbound_rx,
            outbound_tx,
            Arc::clone(&worker_pool),
            run_lock,
        );

        let handler_result = handler.run().await;

        debug!("server: session input closed — shutting down");

        if let Some(watcher_task) = watcher_task {
            watcher_task.abort();
            let _ = watcher_task.await;
        }
        if let Ok(pool) = Arc::try_unwrap(worker_pool) {
            pool.shutdown();
        }
        handler_result
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use std::time::Duration;

    use log::LevelFilter;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf},
        time,
    };
    use tryke_testing::python_bin as test_python_bin;

    use super::*;

    type ClientWriter = WriteHalf<DuplexStream>;
    type ClientReader = BufReader<ReadHalf<DuplexStream>>;

    /// Spawn a server over an in-memory duplex pipe and return the client
    /// halves of the session, mirroring how an editor owns the stdio of a
    /// spawned `tryke server` child.
    fn start_server() -> (ClientWriter, ClientReader, tempfile::TempDir) {
        start_server_inner(None)
    }

    fn start_server_with_manual_file_watcher() -> (
        ClientWriter,
        ClientReader,
        tempfile::TempDir,
        mpsc::UnboundedSender<FileChangeBatch>,
    ) {
        let (changes_tx, changes_rx) = mpsc::unbounded_channel();
        let (writer, reader, directory) = start_server_inner(Some(changes_rx));
        (writer, reader, directory, changes_tx)
    }

    fn start_server_inner(
        manual_changes: Option<mpsc::UnboundedReceiver<FileChangeBatch>>,
    ) -> (ClientWriter, ClientReader, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let root = dir.path().to_path_buf();
        let src_roots = vec![root.clone()];
        let python = test_python_bin();
        let (client, server_side) = tokio::io::duplex(1 << 16);
        let (server_r, server_w) = tokio::io::split(server_side);
        tokio::spawn(async move {
            let worker_pool =
                WorkerPool::spawn(1, &python, &root, None, LevelFilter::Off, false).await;
            let discoverer = Discoverer::new(&root, src_roots, &[], None);
            let server = Server::with_transport(worker_pool, discoverer, server_r, server_w);
            let server = match manual_changes {
                Some(changes) => server.with_manual_file_watcher(changes),
                None => server.without_file_watcher(),
            };
            server.serve().await.expect("server run");
        });
        let (client_r, client_w) = tokio::io::split(client);
        (client_w, BufReader::new(client_r), dir)
    }

    #[tokio::test]
    async fn ping_pong() {
        let (mut w, mut r, _dir) = start_server();
        w.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        let mut line = String::new();
        // Generous timeout for loaded CI hosts. The worker pool starts cold,
        // so ping itself does not wait for Python subprocess startup.
        time::timeout(Duration::from_secs(30), r.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let val: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(val["result"], "pong");
    }

    #[tokio::test]
    async fn stdin_eof_shuts_server_down() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let root = dir.path().to_path_buf();
        let src_roots = vec![root.clone()];
        let python = test_python_bin();
        let (client, server_side) = tokio::io::duplex(1 << 16);
        let (server_r, server_w) = tokio::io::split(server_side);
        let handle = tokio::spawn(async move {
            let worker_pool =
                WorkerPool::spawn(1, &python, &root, None, LevelFilter::Off, false).await;
            let discoverer = Discoverer::new(&root, src_roots, &[], None);
            Server::with_transport(worker_pool, discoverer, server_r, server_w)
                .without_file_watcher()
                .serve()
                .await
        });
        // Closing the client end delivers EOF on the server's input —
        // the LSP-style shutdown signal.
        drop(client);
        let result = time::timeout(Duration::from_secs(30), handle)
            .await
            .expect("server must shut down after EOF")
            .expect("server task must not panic");
        assert!(
            result.is_ok(),
            "server must exit cleanly on EOF: {result:?}"
        );
    }

    fn match_body(value: &str) -> String {
        format!(
            "from tryke import describe, expect, test\n\
             \n\
             def match() -> str:\n\
             {INDENT}return \"{value}\"\n\
             \n\
             with describe(\"match\"):\n\
             {INDENT}@test(\"basic\")\n\
             {INDENT}def basic():\n\
             {INDENT}{INDENT}expect(match()).to_equal(\"set\")\n",
            INDENT = "    ",
        )
    }

    /// Read JSON-RPC lines from `r` until one with an `id` field (the
    /// response — notifications have no `id`).
    async fn read_response(r: &mut ClientReader) -> serde_json::Value {
        loop {
            let mut line = String::new();
            time::timeout(Duration::from_secs(30), r.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            if v.get("id").is_some() {
                return v;
            }
        }
    }

    async fn read_notification(r: &mut ClientReader, method: &str) -> serde_json::Value {
        loop {
            let mut line = String::new();
            time::timeout(Duration::from_secs(30), r.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            if value["method"] == method {
                return value;
            }
            assert!(
                value.get("id").is_none(),
                "received unexpected response while waiting for {method}: {value}",
            );
        }
    }

    /// Send `did_change` then `run` on the SAME session — the invariant
    /// that makes the in-band approach race-free.
    async fn did_change_then_run(
        w: &mut ClientWriter,
        r: &mut ClientReader,
        file: &std::path::Path,
        rid: &str,
    ) -> serde_json::Value {
        // serde_json::to_string handles JSON escaping (Windows
        // backslashes in the path would otherwise produce invalid JSON).
        let mut dc = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "did_change",
            "params": { "paths": [file] },
        }))
        .unwrap();
        dc.push('\n');
        w.write_all(dc.as_bytes()).await.unwrap();
        let _dc_resp = read_response(r).await;

        let run = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"run\",\"params\":{{\"run_id\":\"{rid}\"}}}}\n"
        );
        w.write_all(run.as_bytes()).await.unwrap();
        read_response(r).await
    }

    /// Send `run` only (no `did_change`) — simulates a non-cooperating
    /// client. Used to verify the FS-watcher fallback path.
    async fn run_only(w: &mut ClientWriter, r: &mut ClientReader, rid: &str) -> serde_json::Value {
        let run = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"run\",\"params\":{{\"run_id\":\"{rid}\"}}}}\n"
        );
        w.write_all(run.as_bytes()).await.unwrap();
        read_response(r).await
    }

    /// Regression: a `run` issued *immediately* after a save (no sleep
    /// to let the FS watcher catch up) must see fresh `sys.modules`,
    /// not the previous cycle's cache. The client sends `did_change`
    /// first so the server refreshes discovery synchronously before
    /// the `run` line is read.
    ///
    /// Without the `did_change` step, phase 2 can use stale discovery
    /// metadata even though workers are fresh for every run.
    #[tokio::test]
    async fn run_after_did_change_uses_fresh_module() {
        let (mut w, mut r, dir) = start_server();
        let test_file = dir.path().join("test_match.py");

        fs::write(&test_file, match_body("set")).unwrap();
        let resp = did_change_then_run(&mut w, &mut r, &test_file, "set").await;
        let summary = &resp["result"]["summary"];
        assert_eq!(
            summary["passed"].as_u64().unwrap_or(0),
            1,
            "'set' baseline must pass — got summary={summary}",
        );

        // Flip to "st"; the assertion stays "set", so a fresh import must
        // fail. A pass means the worker served its phase-1 cached module.
        fs::write(&test_file, match_body("st")).unwrap();
        let resp = did_change_then_run(&mut w, &mut r, &test_file, "st").await;
        let summary = &resp["result"]["summary"];
        let passed = summary["passed"].as_u64().unwrap_or(0);
        let failed = summary["failed"].as_u64().unwrap_or(0);
        let errors = summary["errors"].as_u64().unwrap_or(0);
        assert!(
            passed == 0 && (failed + errors) >= 1,
            "file has match()->\"st\" but the run reported passed={passed}; \
             the worker served the stale phase-1 cache. summary={summary}",
        );
    }

    /// A file-change event refreshes discovery for clients that do not send
    /// `did_change`. The event is injected after the platform watcher boundary
    /// so the test remains deterministic across operating systems.
    #[tokio::test]
    async fn manually_triggered_file_change_refreshes_discovery() {
        let (mut w, mut r, dir, changes) = start_server_with_manual_file_watcher();
        let test_file = dir.path().join("test_match.py");

        w.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        let _ = read_response(&mut r).await;

        fs::write(&test_file, match_body("set")).unwrap();
        changes
            .send(FileChangeBatch {
                paths: vec![test_file.clone()],
            })
            .expect("send initial file change");
        let _ = read_notification(&mut r, "discover_complete").await;
        let resp = run_only(&mut w, &mut r, "warm").await;
        assert_eq!(
            resp["result"]["summary"]["passed"].as_u64().unwrap_or(0),
            1,
            "warm-up: 'set' body must pass — got {}",
            resp["result"]["summary"]
        );

        fs::write(&test_file, match_body("st")).unwrap();
        changes
            .send(FileChangeBatch {
                paths: vec![test_file],
            })
            .expect("send updated file change");
        let _ = read_notification(&mut r, "discover_complete").await;
        let resp = run_only(&mut w, &mut r, "after_save").await;
        let summary = &resp["result"]["summary"];
        let passed = summary["passed"].as_u64().unwrap_or(0);
        let failed = summary["failed"].as_u64().unwrap_or(0);
        assert!(
            passed == 0 && failed >= 1,
            "after the file-change event: 'st' body should fail the 'set' assertion — \
             got summary={summary}",
        );
    }

    #[tokio::test]
    async fn repeated_runs_reexecute_module_imports() {
        let (mut w, mut r, dir) = start_server();
        let test_file = dir.path().join("test_import.py");
        let counter_file = dir.path().join("imports.txt");
        let counter_literal = serde_json::to_string(&counter_file).expect("serialize counter path");
        fs::write(
            &test_file,
            format!(
                "from pathlib import Path\n\
                 from tryke import test\n\
                 \n\
                 counter = Path({counter_literal})\n\
                 previous = counter.read_text() if counter.exists() else \"\"\n\
                 counter.write_text(previous + \"x\")\n\
                 \n\
                 @test\n\
                 def test_import():\n\
                 {INDENT}pass\n",
                INDENT = "    ",
            ),
        )
        .expect("write test file");

        let first = did_change_then_run(&mut w, &mut r, &test_file, "first").await;
        assert_eq!(first["result"]["summary"]["passed"], 1);
        assert_eq!(
            fs::read_to_string(&counter_file).expect("read import counter"),
            "x",
        );

        let second = run_only(&mut w, &mut r, "second").await;
        assert_eq!(second["result"]["summary"]["passed"], 1);
        assert_eq!(
            fs::read_to_string(&counter_file).expect("read import counter"),
            "xx",
            "each logical run must import test modules in a fresh Python process",
        );
    }
}
