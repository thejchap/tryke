use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use bytes::Bytes;
use log::{LevelFilter, debug};
use tokio::{
    net::TcpListener,
    sync::{Mutex, broadcast},
};
use tryke_discovery::Discoverer;
use tryke_runner::WorkerPool;

use crate::{change::apply_change, handler::ConnectionHandler, watcher::spawn_watcher};

pub struct Server {
    port: u16,
    root: PathBuf,
    excludes: Vec<String>,
    cache_dir: Option<PathBuf>,
    python: String,
    log_level: LevelFilter,
}

impl Server {
    #[must_use]
    pub fn new(
        port: u16,
        root: PathBuf,
        excludes: Vec<String>,
        python: String,
        log_level: LevelFilter,
        cache_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            port,
            root,
            excludes,
            cache_dir,
            python,
            log_level,
        }
    }

    #[expect(clippy::missing_errors_doc)]
    pub async fn run(self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", self.port)).await?;
        self.run_on_listener(listener).await
    }

    #[expect(clippy::missing_errors_doc)]
    pub async fn run_on_listener(self, listener: TcpListener) -> anyhow::Result<()> {
        let size = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
        let pool = Arc::new(WorkerPool::new(
            size,
            &self.python,
            &self.root,
            self.log_level,
        ));
        pool.warm().await;

        // Held across the full duration of a single `run` so concurrent
        // clients don't interleave test execution on the shared pool.
        let run_lock = Arc::new(Mutex::new(()));

        // Set by the watcher whenever a real file change is accepted;
        // drained by `execute_run` under `run_lock` to force a worker pool
        // restart before any unit is dispatched. Without this, a `run`
        // request that arrives within the watcher's debounce window can
        // be served by a worker whose cached `sys.modules` predates the
        // on-disk content — the test then runs against stale module
        // globals (the original symptom: server mode flakily honours a
        // just-edited assertion).
        let dirty = Arc::new(AtomicBool::new(false));

        let (bcast_tx, _) = broadcast::channel::<Bytes>(256);
        let disc = Arc::new(Mutex::new(Discoverer::new_with_excludes_and_cache_dir(
            &self.root,
            &self.excludes,
            self.cache_dir.as_deref(),
        )));
        disc.lock().await.rediscover();

        let (std_tx, std_rx) = std::sync::mpsc::channel::<Vec<std::path::PathBuf>>();
        let _debouncer = spawn_watcher(&self.root, &self.excludes, std_tx)?;

        // Bridge blocking std receiver to async tokio channel
        let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel::<Vec<PathBuf>>(64);
        tokio::task::spawn_blocking(move || {
            while let Ok(paths) = std_rx.recv() {
                let _ = watcher_tx.blocking_send(paths);
            }
        });

        let disc_for_watcher = Arc::clone(&disc);
        let bcast_for_watcher = bcast_tx.clone();
        let dirty_for_watcher = Arc::clone(&dirty);
        let mut change_filter = crate::watcher::ChangeFilter::new();
        tokio::spawn(async move {
            while let Some(first) = watcher_rx.recv().await {
                // Coalesce any additional batches already queued. A
                // single editor save can produce events whose quiet
                // windows straddle the watcher's debounce, yielding two
                // back-to-back batches. Without this drain we would
                // restart the worker pool twice for one save.
                let mut paths = first;
                while let Ok(more) = watcher_rx.try_recv() {
                    paths.extend(more);
                }
                paths.sort();
                paths.dedup();

                // Drop paths whose `(mtime, size)` hasn't actually
                // moved since the last accepted batch. Drain handles
                // simultaneously-queued batches; this handles tail
                // events that arrive after the previous cycle.
                let paths = change_filter.filter(&paths);
                if paths.is_empty() {
                    debug!("server: file change batch had no real content changes — skipping");
                    continue;
                }

                // Shared with the `did_change` RPC handler (handler.rs)
                // so the FS path and the in-band path stay semantically
                // identical. The next `run` will drain `dirty` and
                // restart the worker pool before dispatch.
                apply_change(
                    &disc_for_watcher,
                    &bcast_for_watcher,
                    &dirty_for_watcher,
                    &paths,
                )
                .await;
            }
        });

        loop {
            let (stream, addr) = listener.accept().await?;
            debug!("accepted connection from {addr}");
            let bcast_rx = bcast_tx.subscribe();
            let bcast_tx_conn = bcast_tx.clone();
            let disc_conn = Arc::clone(&disc);
            let pool_conn = Arc::clone(&pool);
            let run_lock_conn = Arc::clone(&run_lock);
            let dirty_conn = Arc::clone(&dirty);
            tokio::spawn(async move {
                ConnectionHandler::new(
                    stream,
                    disc_conn,
                    bcast_rx,
                    bcast_tx_conn,
                    pool_conn,
                    run_lock_conn,
                    dirty_conn,
                )
                .run()
                .await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use std::time::Duration;

    use log::LevelFilter;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream},
        time,
    };
    use tryke_testing::python_bin as test_python_bin;

    use super::*;

    async fn start_server() -> (u16, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let root = dir.path().to_path_buf();
        let python = test_python_bin();
        tokio::spawn(async move {
            Server::new(port, root, vec![], python, LevelFilter::Off, None)
                .run_on_listener(listener)
                .await
                .unwrap();
        });
        (port, dir)
    }

    #[tokio::test]
    async fn ping_pong() {
        let (port, _dir) = start_server().await;
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let val: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(val["result"], "pong");
    }

    #[tokio::test]
    async fn multi_client_both_receive_broadcast() {
        let (port, _dir) = start_server().await;

        let mut c1 = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut c2 = TcpStream::connect(("127.0.0.1", port)).await.unwrap();

        let run_req = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"run\",\"params\":{\"root\":\"/ignored\",\"tests\":null,\"run_id\":\"r1\"}}\n";
        c1.write_all(run_req.as_bytes()).await.unwrap();

        let mut r2 = BufReader::new(&mut c2);
        let mut line2 = String::new();
        time::timeout(Duration::from_secs(5), r2.read_line(&mut line2))
            .await
            .unwrap()
            .unwrap();
        let v2: serde_json::Value = serde_json::from_str(line2.trim()).unwrap();
        assert!(v2.get("method").is_some(), "c2 should receive a broadcast");
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
    async fn read_response(r: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> serde_json::Value {
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

    /// Send `did_change` then `run` on the SAME TCP connection — the
    /// invariant that makes the in-band approach race-free.
    async fn did_change_then_run(
        port: u16,
        file: &std::path::Path,
        rid: &str,
    ) -> serde_json::Value {
        let s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (read, mut write) = s.into_split();
        let mut r = BufReader::new(read);

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
        write.write_all(dc.as_bytes()).await.unwrap();
        let _dc_resp = read_response(&mut r).await;

        let run = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"run\",\"params\":{{\"run_id\":\"{rid}\"}}}}\n"
        );
        write.write_all(run.as_bytes()).await.unwrap();
        read_response(&mut r).await
    }

    /// Send `run` only (no `did_change`) — simulates a non-cooperating
    /// client. Used to verify the FS-watcher fallback path.
    async fn run_only(port: u16, rid: &str) -> serde_json::Value {
        let s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (read, mut write) = s.into_split();
        let mut r = BufReader::new(read);
        let run = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"run\",\"params\":{{\"run_id\":\"{rid}\"}}}}\n"
        );
        write.write_all(run.as_bytes()).await.unwrap();
        read_response(&mut r).await
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
        let (port, dir) = start_server().await;
        let test_file = dir.path().join("test_match.py");

        fs::write(&test_file, match_body("set")).unwrap();
        // One settling pause so the *initial* discovery picks up the
        // file. After this, every iteration uses `did_change` and does
        // not wait.
        time::sleep(Duration::from_millis(300)).await;

        // Phase 1 — every worker imports the "set" body.
        for i in 0..16 {
            fs::write(&test_file, match_body("set")).unwrap();
            let resp = did_change_then_run(port, &test_file, &format!("set{i}")).await;
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
            let resp = did_change_then_run(port, &test_file, &format!("st{i}")).await;
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
        let (port, dir) = start_server().await;
        let test_file = dir.path().join("test_match.py");

        fs::write(&test_file, match_body("set")).unwrap();
        time::sleep(Duration::from_millis(300)).await;
        let resp = run_only(port, "warm").await;
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
        let resp = run_only(port, "after_save").await;
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
