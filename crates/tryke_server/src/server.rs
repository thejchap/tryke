use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use log::debug;
use tokio::{
    io::{AsyncRead, AsyncWrite, Stdin, Stdout},
    sync::{Mutex, mpsc},
};
use tryke_discovery::Discoverer;
use tryke_runner::WorkerPool;

use crate::{change::apply_change, handler::ConnectionHandler, watcher::spawn_watcher};

pub struct Server<R, W> {
    reader: R,
    writer: W,
    worker_pool: WorkerPool,
    discoverer: Discoverer,
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
        }
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
        } = self;

        let root = discoverer.root().to_path_buf();
        let excludes = discoverer.excludes().to_vec();
        let worker_pool = Arc::new(worker_pool);
        let run_lock = Arc::new(Mutex::new(()));
        let dirty = Arc::new(AtomicBool::new(false));

        // Everything sent to the client goes through this queue
        let (outbound_tx, outbound_rx) = mpsc::channel(256);

        // Initialize the discoverer and do its initial discovery/populate import graph/test cache
        let discoverer = Arc::new(Mutex::new(discoverer));
        discoverer.lock().await.rediscover();

        let (std_tx, std_rx) = std::sync::mpsc::channel::<Vec<std::path::PathBuf>>();
        let debouncer = spawn_watcher(&root, &excludes, std_tx)?;
        let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel::<Vec<PathBuf>>(64);
        tokio::task::spawn_blocking(move || {
            while let Ok(paths) = std_rx.recv() {
                if watcher_tx.blocking_send(paths).is_err() {
                    break;
                }
            }
        });
        let disc_for_watcher = Arc::clone(&discoverer);
        let outbound_for_watcher = outbound_tx.clone();
        let dirty_for_watcher = Arc::clone(&dirty);
        let mut change_filter = crate::watcher::ChangeFilter::new();
        tokio::spawn(async move {
            while let Some(first) = watcher_rx.recv().await {
                let mut paths = first;
                while let Ok(more) = watcher_rx.try_recv() {
                    paths.extend(more);
                }
                paths.sort();
                paths.dedup();
                let paths = change_filter.filter(&paths);
                if paths.is_empty() {
                    debug!("server: file change batch had no real content changes — skipping");
                    continue;
                }
                if let Err(error) = apply_change(
                    &disc_for_watcher,
                    &outbound_for_watcher,
                    &dirty_for_watcher,
                    &paths,
                )
                .await
                {
                    debug!("server: stopping file-change notifications: {error}");
                    break;
                }
            }
        });

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
            dirty,
        );

        let handler_result = handler.run().await;

        debug!("server: session input closed — shutting down");

        drop(debouncer);
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
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let root = dir.path().to_path_buf();
        let src_roots = vec![root.clone()];
        let python = test_python_bin();
        let (client, server_side) = tokio::io::duplex(1 << 16);
        let (server_r, server_w) = tokio::io::split(server_side);
        tokio::spawn(async move {
            let worker_pool =
                WorkerPool::spawn(1, &python, &root, None, LevelFilter::Off, true).await;
            let discoverer = Discoverer::new(&root, src_roots, &[], None);
            Server::with_transport(worker_pool, discoverer, server_r, server_w)
                .serve()
                .await
                .expect("server run");
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
        // Generous timeout: the response is gated behind Python worker
        // startup and initial discovery, which can be slow on loaded CI hosts.
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
                WorkerPool::spawn(1, &python, &root, None, LevelFilter::Off, true).await;
            let discoverer = Discoverer::new(&root, src_roots, &[], None);
            Server::with_transport(worker_pool, discoverer, server_r, server_w)
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
    /// not the previous cycle's cache. Drives the same phase-1/phase-2
    /// shape that catches stale-cache leaks across 16 workers — but
    /// this time the client sends `did_change` first so the server can
    /// flip `dirty` synchronously before the `run` line is read.
    ///
    /// Without the `did_change` step (or without the conditional drain
    /// in `execute_run`), phase 2 sees passing runs because workers
    /// dispatched against their phase-1 cached "set" body even though
    /// the file is now "st" on disk.
    #[tokio::test]
    async fn run_after_did_change_uses_fresh_module() {
        let (mut w, mut r, dir) = start_server();
        let test_file = dir.path().join("test_match.py");

        fs::write(&test_file, match_body("set")).unwrap();
        // One settling pause so the *initial* discovery picks up the
        // file. After this, every iteration uses `did_change` and does
        // not wait.
        time::sleep(Duration::from_millis(300)).await;

        // Phase 1 — every worker imports the "set" body.
        for i in 0..16 {
            fs::write(&test_file, match_body("set")).unwrap();
            let resp = did_change_then_run(&mut w, &mut r, &test_file, &format!("set{i}")).await;
            let summary = &resp["result"]["summary"];
            let passed = summary["passed"].as_u64().unwrap_or(0);
            assert_eq!(
                passed, 1,
                "phase 1 iter {i}: 'set' baseline must pass — got summary={summary}",
            );
        }

        // Phase 2 — flip to "st"; assertion stays "set", so every fresh
        // import must FAIL. A `passed=1` here means a worker served its
        // phase-1 cached module: exactly the staleness `did_change` →
        // `dirty` → drain is here to prevent.
        for i in 0..16 {
            fs::write(&test_file, match_body("st")).unwrap();
            let resp = did_change_then_run(&mut w, &mut r, &test_file, &format!("st{i}")).await;
            let summary = &resp["result"]["summary"];
            let passed = summary["passed"].as_u64().unwrap_or(0);
            let failed = summary["failed"].as_u64().unwrap_or(0);
            let errors = summary["errors"].as_u64().unwrap_or(0);
            assert!(
                passed == 0 && (failed + errors) >= 1,
                "phase 2 iter {i}: file has match()->\"st\" but the run \
                 reported passed={passed}; a worker served the stale \
                 phase-1 cache. summary={summary}",
            );
        }
    }

    /// Companion to the test above: a non-cooperating client (just
    /// sends `run`, no `did_change`) still eventually gets correct
    /// results once the FS watcher's 50 ms debounce expires. This
    /// documents the fallback path and ensures we haven't accidentally
    /// removed it.
    #[tokio::test]
    async fn run_only_falls_back_to_fs_watcher() {
        let (mut w, mut r, dir) = start_server();
        let test_file = dir.path().join("test_match.py");

        fs::write(&test_file, match_body("set")).unwrap();
        time::sleep(Duration::from_millis(300)).await;
        let resp = run_only(&mut w, &mut r, "warm").await;
        assert_eq!(
            resp["result"]["summary"]["passed"].as_u64().unwrap_or(0),
            1,
            "warm-up: 'set' body must pass — got {}",
            resp["result"]["summary"]
        );

        fs::write(&test_file, match_body("st")).unwrap();
        // Give the FS watcher its debounce + a comfortable margin to
        // process the change and mark dirty. Non-cooperating clients
        // accept this latency.
        time::sleep(Duration::from_millis(300)).await;
        let resp = run_only(&mut w, &mut r, "after_save").await;
        let summary = &resp["result"]["summary"];
        let passed = summary["passed"].as_u64().unwrap_or(0);
        let failed = summary["failed"].as_u64().unwrap_or(0);
        assert!(
            passed == 0 && failed >= 1,
            "after FS-watcher debounce: 'st' body should fail the 'set' assertion — \
             got summary={summary}",
        );
    }
}
