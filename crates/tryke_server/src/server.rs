use std::{path::PathBuf, sync::Arc};

use bytes::Bytes;
use log::debug;
use tokio::{
    net::TcpListener,
    sync::{Mutex, broadcast},
};
use tryke_discovery::Discoverer;
use tryke_runner::{WorkerPool, check_python_version, resolve_python};

use crate::{
    handler::ConnectionHandler,
    protocol::{DiscoverCompleteParams, Notification},
    watcher::spawn_watcher,
};

pub struct Server {
    port: u16,
    root: PathBuf,
    excludes: Vec<String>,
}

impl Server {
    #[must_use]
    pub fn new(port: u16, root: PathBuf, excludes: Vec<String>) -> Self {
        Self {
            port,
            root,
            excludes,
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
        let python = resolve_python(&self.root);
        check_python_version(&python, &self.root)?;
        let pool = Arc::new(WorkerPool::new(size, &python, &self.root));
        pool.warm().await;

        // Held across the full duration of a single `run` so concurrent
        // clients don't interleave test execution on the shared pool.
        let run_lock = Arc::new(Mutex::new(()));

        let (bcast_tx, _) = broadcast::channel::<Bytes>(256);
        let disc = Arc::new(Mutex::new(Discoverer::new_with_excludes(
            &self.root,
            &self.excludes,
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
        let pool_for_watcher = Arc::clone(&pool);
        tokio::spawn(async move {
            while let Some(paths) = watcher_rx.recv().await {
                let (modules, tests) = {
                    let mut disc = disc_for_watcher.lock().await;
                    let modules = disc.affected_modules(&paths);
                    disc.rediscover_changed(&paths);
                    let tests = disc.tests_for_changed(&paths);
                    (modules, tests)
                };
                if modules.is_empty() {
                    debug!("server: no modules affected by change — skipping pool.reload");
                } else {
                    debug!(
                        "server: reloading {} module(s) in worker pool: {}",
                        modules.len(),
                        modules.join(", ")
                    );
                    pool_for_watcher.reload(modules).await;
                }
                debug!("file change: {} affected tests", tests.len());
                let notif = Notification {
                    jsonrpc: "2.0".to_string(),
                    method: "discover_complete".to_string(),
                    params: DiscoverCompleteParams { tests },
                };
                if let Ok(mut bytes) = serde_json::to_vec(&notif) {
                    bytes.push(b'\n');
                    let _ = bcast_for_watcher.send(Bytes::from(bytes));
                }
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
            tokio::spawn(async move {
                ConnectionHandler::new(
                    stream,
                    disc_conn,
                    bcast_rx,
                    bcast_tx_conn,
                    pool_conn,
                    run_lock_conn,
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

    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream},
        time,
    };

    use super::*;

    async fn start_server() -> (u16, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let root = dir.path().to_path_buf();
        tokio::spawn(async move {
            Server::new(port, root, vec![])
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

        let run_req = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"run\",\"params\":{\"root\":\"/ignored\",\"tests\":null}}\n";
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
}
