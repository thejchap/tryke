//! End-to-end test of `tryke server` speaking newline-delimited JSON-RPC
//! 2.0 over the real binary's stdin/stdout, the way an editor plugin owns
//! a spawned server child.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

fn match_body() -> &'static str {
    "from tryke import describe, expect, test\n\
     \n\
     with describe(\"match\"):\n\
     \x20   @test(\"basic\")\n\
     \x20   def basic():\n\
     \x20       expect(1).to_equal(1)\n"
}

struct ServerSession {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    _dir: tempfile::TempDir,
}

impl ServerSession {
    fn spawn() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        fs::write(dir.path().join("test_match.py"), match_body()).expect("write test file");

        let mut child = Command::new(env!("CARGO_BIN_EXE_tryke"))
            .args(["server", "--root"])
            .arg(dir.path())
            .args(["--python", &tryke_testing::python_bin()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn tryke server");

        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("child stdout piped");
        // A reader thread decouples blocking pipe reads from the test's
        // per-line timeouts. It exits on EOF when the server shuts down.
        let (tx, rx) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        Self {
            child,
            stdin,
            lines: rx,
            _dir: dir,
        }
    }

    fn send(&mut self, request: &str) {
        let stdin = self.stdin.as_mut().expect("stdin still open");
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .expect("write request");
        stdin.flush().expect("flush request");
    }

    /// Read frames until one carries an `id` (the response — notifications
    /// have none). Every frame on stdout MUST parse as JSON: a single
    /// stray print would corrupt the protocol for real clients.
    fn read_response(&self) -> serde_json::Value {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(60))
                .expect("server went quiet before sending a response");
            let value: serde_json::Value = serde_json::from_str(line.trim())
                .unwrap_or_else(|e| panic!("non-JSON frame on stdout ({e}): {line:?}"));
            if value.get("id").is_some() {
                return value;
            }
        }
    }

    /// Close stdin (EOF) and wait for the child to exit.
    fn close_and_wait(&mut self) -> std::process::ExitStatus {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "server did not exit within 30s of stdin EOF"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for ServerSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn server_speaks_json_rpc_over_stdio() {
    let mut session = ServerSession::spawn();

    // Ping → pong.
    session.send(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    let pong = session.read_response();
    assert_eq!(pong["result"], "pong", "unexpected ping response: {pong}");

    // Run → summary with the one passing test, echoing our run_id.
    session.send(r#"{"jsonrpc":"2.0","id":2,"method":"run","params":{"run_id":"e2e"}}"#);
    let run = session.read_response();
    assert_eq!(
        run["result"]["run_id"], "e2e",
        "unexpected run response: {run}"
    );
    assert_eq!(
        run["result"]["summary"]["passed"], 1,
        "expected exactly one passing test: {run}"
    );

    // EOF on stdin shuts the server down cleanly (exit code 0).
    let status = session.close_and_wait();
    assert!(
        status.success(),
        "server must exit cleanly on EOF: {status}"
    );
}
