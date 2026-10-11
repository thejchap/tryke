//! End-to-end test of `tryke server` speaking newline-delimited JSON-RPC
//! 2.0 over the real binary's stdin/stdout, the way an editor plugin owns
//! a spawned server child.

use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use tryke_testing::TestProject;

fn match_body() -> &'static str {
    "from tryke import describe, expect, test\n\
     \n\
     with describe(\"match\"):\n\
     \x20   @test(\"basic\")\n\
     \x20   def basic():\n\
     \x20       expect(1).to_equal(1)\n"
}

#[cfg(unix)]
/// One test that atomically publishes its worker PID and, while a `hold`
/// file exists at the project root, blocks so signals interrupt it mid-run.
fn pid_marker_body() -> &'static str {
    "import os\n\
     import pathlib\n\
     import time\n\
     \n\
     from tryke import test\n\
     \n\
     ROOT = pathlib.Path(__file__).resolve().parent\n\
     \n\
     \n\
     @test(\"publishes pid\")\n\
     def publishes_pid():\n\
     \x20   tmp = ROOT / \"worker.pid.tmp\"\n\
     \x20   tmp.write_text(str(os.getpid()))\n\
     \x20   os.replace(tmp, ROOT / \"worker.pid\")\n\
     \x20   hold = ROOT / \"hold\"\n\
     \x20   deadline = time.monotonic() + 30\n\
     \x20   while hold.exists() and time.monotonic() < deadline:\n\
     \x20       time.sleep(0.01)\n"
}

struct ServerSession {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    _project: TestProject,
    worker_pid: Option<u32>,
}

impl ServerSession {
    fn spawn() -> io::Result<Self> {
        Self::spawn_for(
            "server",
            TestProject::with_files([("test_match.py", match_body())])?,
        )
    }

    fn spawn_for(command: &str, project: TestProject) -> io::Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tryke"))
            .args([command, "--root"])
            .arg(project.root())
            .args(["--python", &tryke_testing::python_bin()])
            .args(["--workers", "1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("child stdout was not piped"))?;
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

        Ok(Self {
            child,
            stdin,
            lines: rx,
            _project: project,
            worker_pid: None,
        })
    }

    #[cfg(unix)]
    /// Wait for the fixture's atomically written `worker.pid` and record it
    /// so failure cleanup can kill a worker the parent leaked.
    fn wait_for_worker_pid(&mut self) -> io::Result<u32> {
        let marker = self._project.root().join("worker.pid");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match std::fs::read_to_string(&marker) {
                Ok(contents) => {
                    let pid = contents.trim().parse::<u32>().map_err(|error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("invalid worker PID {contents:?}: {error}"),
                        )
                    })?;
                    self.worker_pid = Some(pid);
                    return Ok(pid);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "worker did not publish its PID within 30s",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    /// Wait up to 5s for the parent to exit after a signal.
    fn wait_for_exit(&mut self) -> io::Result<std::process::ExitStatus> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "parent did not exit within 5s of the signal",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn send(&mut self, request: &str) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "server stdin is closed"))?;
        stdin.write_all(format!("{request}\n").as_bytes())?;
        stdin.flush()
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
    fn close_and_wait(&mut self) -> io::Result<std::process::ExitStatus> {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
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
        // Never leave a blocked worker behind when an assertion fails.
        if let Some(pid) = self.worker_pid
            && process_alive(pid)
        {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    }
}

fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(unix)]
fn pid_marker_project(hold: bool) -> io::Result<TestProject> {
    let project = TestProject::with_files([("test_pid.py", pid_marker_body())])?;
    if hold {
        project.write("hold", "")?;
    }
    Ok(project)
}

#[cfg(unix)]
/// SIGTERM the parent, then require exit status 130 within 5s and that the
/// recorded worker no longer exists.
fn assert_sigterm_reaps_worker(session: &mut ServerSession, worker_pid: u32) -> io::Result<()> {
    assert!(
        process_alive(worker_pid),
        "worker {worker_pid} must be alive before SIGTERM"
    );
    let signal = Command::new("kill")
        .args(["-TERM", &session.child.id().to_string()])
        .status()?;
    assert!(signal.success(), "send SIGTERM to parent: {signal}");

    let status = session.wait_for_exit()?;
    assert_eq!(status.code(), Some(130), "terminated parent: {status}");
    assert!(
        !process_alive(worker_pid),
        "worker {worker_pid} outlived its parent after SIGTERM"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn server_sigterm_reaps_idle_worker() -> io::Result<()> {
    let mut session = ServerSession::spawn_for("server", pid_marker_project(false)?)?;
    session.send(r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"run_id":"idle"}}"#)?;
    let run = session.read_response();
    assert_eq!(
        run["result"]["summary"]["passed"], 1,
        "expected exactly one passing test: {run}"
    );
    let worker_pid = session.wait_for_worker_pid()?;
    assert_sigterm_reaps_worker(&mut session, worker_pid)
}

#[cfg(unix)]
#[test]
fn server_sigterm_reaps_active_worker() -> io::Result<()> {
    let mut session = ServerSession::spawn_for("server", pid_marker_project(true)?)?;
    session.send(r#"{"jsonrpc":"2.0","id":1,"method":"run","params":{"run_id":"active"}}"#)?;
    let worker_pid = session.wait_for_worker_pid()?;
    assert_sigterm_reaps_worker(&mut session, worker_pid)
}

#[cfg(unix)]
#[test]
fn test_sigterm_reaps_active_worker() -> io::Result<()> {
    let mut session = ServerSession::spawn_for("test", pid_marker_project(true)?)?;
    let worker_pid = session.wait_for_worker_pid()?;
    assert_sigterm_reaps_worker(&mut session, worker_pid)
}

#[test]
fn server_speaks_json_rpc_over_stdio() -> io::Result<()> {
    let mut session = ServerSession::spawn()?;

    // Ping → pong.
    session.send(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)?;
    let pong = session.read_response();
    assert_eq!(pong["result"], "pong", "unexpected ping response: {pong}");

    // Run → summary with the one passing test, echoing our run_id.
    session.send(r#"{"jsonrpc":"2.0","id":2,"method":"run","params":{"run_id":"e2e"}}"#)?;
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
    let status = session.close_and_wait()?;
    assert!(
        status.success(),
        "server must exit cleanly on EOF: {status}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn server_exits_on_sigint_with_stdin_open() -> io::Result<()> {
    let mut session = ServerSession::spawn()?;
    session.send(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)?;
    let pong = session.read_response();
    assert_eq!(pong["result"], "pong", "unexpected ping response: {pong}");

    let signal = Command::new("kill")
        .args(["-INT", &session.child.id().to_string()])
        .status()?;
    assert!(signal.success(), "send SIGINT to server: {signal}");

    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = session.child.try_wait()? {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "server did not exit after SIGINT while client stdin stayed open"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130), "interrupted server: {status}");
    Ok(())
}
