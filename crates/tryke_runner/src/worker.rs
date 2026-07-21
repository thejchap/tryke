use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use log::{debug, trace};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tryke_types::{TestItem, TestResult, convert_wire_result};

use crate::protocol::{
    FinalizeHooksParams, RPCRequest, RPCRequestMethod, RPCResponse, RegisterHooksParams,
    RunDoctestParams, RunTestParams, RunTestResultWire,
};

/// Cap on retained worker-stderr bytes. Beyond this we keep the most recent
/// bytes and drop older ones — enough for diagnostic context on failures
/// without unbounded memory growth on workers that spew warnings.
const STDERR_RETAIN_BYTES: usize = 1 << 20; // 1 MiB

pub struct WorkerProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    /// Continuously-drained worker stderr. A background task reads the
    /// child's stderr pipe into this buffer so the pipe never fills and
    /// the worker can't block on a stderr write mid-RPC.
    stderr_buf: Arc<Mutex<VecDeque<u8>>>,
    /// Handle to the stderr-drainer task. `drain_stderr` joins this
    /// (with a short timeout) so any bytes still in the kernel pipe at
    /// the moment of a worker failure end up in `stderr_buf` before we
    /// snapshot it — without this, a worker that dies during startup
    /// can lose its python traceback to a race with the RPC error path.
    stderr_drainer: Option<tokio::task::JoinHandle<()>>,
    next_id: u64,
}

impl WorkerProcess {
    /// Spawn a fresh worker process.
    ///
    /// `log_level` is forwarded as `TRYKE_LOG=<level>` on the child env so
    /// the python worker's `_configure_logging_from_env` lights up at the
    /// same level as the rust process. Pass `LevelFilter::Off` to keep the
    /// worker silent (no env var set), preserving the pre-existing
    /// "no chatter unless asked" default.
    ///
    /// # Errors
    /// Returns an error if the Python process cannot be spawned, if its stdio
    /// pipes cannot be captured, or if the stderr drainer cannot be started.
    pub fn spawn(
        python_bin: &str,
        python_path: &[&Path],
        root: &Path,
        log_level: log::LevelFilter,
    ) -> Result<Self> {
        debug!("spawning worker: {python_bin} -m tryke.worker (log={log_level})");
        let pythonpath = build_pythonpath(python_path);
        let mut command = Command::new(python_bin);
        command
            .args(["-m", "tryke.worker"])
            .env("PYTHONPATH", &pythonpath)
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(value) = worker_log_env_value(log_level) {
            command.env("TRYKE_LOG", value);
        }
        let mut child = command.spawn()?;
        let stdin = BufWriter::new(child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?);
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
        debug!("worker spawned (pid {:?})", child.id());

        // The worker can write to stderr at any time (asyncio default
        // exception handler, library warnings, etc.). The kernel pipe
        // buffer defaults to 64 KiB on Linux, so without an active
        // reader the worker's next stderr write blocks once the pipe
        // fills — and the worker stops responding to RPCs, surfacing
        // as "tryke hangs at finalize_hooks". Spawn a drainer that
        // keeps the pipe empty for the worker's lifetime.
        let stderr_buf = Arc::new(Mutex::new(VecDeque::<u8>::new()));
        let stderr_drainer = match spawn_stderr_drainer(stderr, Arc::clone(&stderr_buf)) {
            Ok(handle) => handle,
            Err(err) => {
                if let Err(kill_err) = child.start_kill() {
                    debug!(
                        "failed to kill worker after stderr drainer setup error (pid {:?}): \
                         {kill_err}",
                        child.id()
                    );
                }
                return Err(err);
            }
        };

        Ok(Self {
            child,
            stdin,
            stdout,
            stderr_buf,
            stderr_drainer: Some(stderr_drainer),
            next_id: 1,
        })
    }

    async fn call<R: for<'de> serde::Deserialize<'de>>(
        &mut self,
        method: RPCRequestMethod,
        params: Option<serde_json::Value>,
    ) -> Result<R> {
        let id = self.next_id;
        self.next_id += 1;
        let req = RPCRequest {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        trace!("worker rpc -> {}", line.trim());
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        // Read lines from stdout, skipping non-JSON garbage that a native
        // library may have written to fd 1 during import (e.g. weasyprint via
        // cffi).  Collect leaked lines so we can surface them in errors.
        let mut leaked_stdout: Vec<String> = Vec::new();
        let resp: RPCResponse = loop {
            let mut resp_line = String::new();
            let n = self.stdout.read_line(&mut resp_line).await?;
            if n == 0 {
                trace!("worker rpc: stdout EOF");
                return Err(if leaked_stdout.is_empty() {
                    anyhow!("worker process closed stdout")
                } else {
                    anyhow!(
                        "worker process closed stdout after writing non-JSON output \
                         (a library may have written to stdout during import):\n{}",
                        leaked_stdout.join("")
                    )
                });
            }
            trace!("worker rpc <- {}", resp_line.trim());
            let trimmed = resp_line.trim();
            if !trimmed.is_empty()
                && let Ok(resp) = serde_json::from_str::<RPCResponse>(trimmed)
            {
                if !leaked_stdout.is_empty() {
                    trace!(
                        "worker rpc: skipped {} non-JSON line(s) on stdout",
                        leaked_stdout.len()
                    );
                }
                break resp;
            }
            leaked_stdout.push(resp_line);
            if leaked_stdout.len() >= 50 {
                return Err(anyhow!(
                    "expected JSON-RPC response from worker but got {} lines of \
                     non-JSON output (a library may have written to stdout during \
                     import):\n{}",
                    leaked_stdout.len(),
                    leaked_stdout.join("")
                ));
            }
        };
        if let Some(err) = resp.error {
            let detail = if let Some(tb) = &err.traceback {
                format!("rpc error {}: {}\n{tb}", err.code, err.message)
            } else {
                format!("rpc error {}: {}", err.code, err.message)
            };
            return Err(anyhow!(detail));
        }
        let val = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(serde_json::from_value(val)?)
    }

    /// Run a discovered test or doctest in the worker process.
    ///
    /// # Errors
    /// Returns an error if the request cannot be serialized, if worker I/O
    /// fails, or if the worker returns a JSON-RPC error.
    pub async fn run_test(&mut self, test: &TestItem) -> Result<TestResult> {
        if let Some(object_path) = &test.doctest_object {
            return self.run_doctest(test, object_path).await;
        }
        let params = serde_json::to_value(RunTestParams {
            module: test.module_path.clone(),
            function: test.name.clone(),
            xfail: test.xfail.clone(),
            groups: test.groups.clone(),
            case_label: test.case_label.clone(),
        })?;
        let wire: RunTestResultWire = self.call(RPCRequestMethod::RunTest, Some(params)).await?;
        Ok(convert_wire_result(test.clone(), wire))
    }

    /// Send hook metadata for a module to the Python worker.
    /// Must be called before running any tests from that module.
    ///
    /// # Errors
    /// Returns an error if hook metadata cannot be serialized or the worker
    /// rejects the registration request.
    pub async fn register_hooks(&mut self, params: RegisterHooksParams) -> Result<()> {
        let value = serde_json::to_value(params)?;
        self.call::<serde_json::Value>(RPCRequestMethod::RegisterHooks, Some(value))
            .await?;
        Ok(())
    }

    /// Tell the Python worker to run scope-level teardown for `per="scope"`
    /// fixtures in a module. Must be called after all tests from that module
    /// have run.
    ///
    /// # Errors
    /// Returns an error if the finalize request cannot be serialized or the
    /// worker reports a teardown failure.
    pub async fn finalize_hooks(&mut self, module: String) -> Result<()> {
        let value = serde_json::to_value(FinalizeHooksParams { module })?;
        self.call::<serde_json::Value>(RPCRequestMethod::FinalizeHooks, Some(value))
            .await?;
        Ok(())
    }

    async fn run_doctest(&mut self, test: &TestItem, object_path: &str) -> Result<TestResult> {
        let params = serde_json::to_value(RunDoctestParams {
            module: test.module_path.clone(),
            object_path: object_path.to_owned(),
        })?;
        let wire: RunTestResultWire = self
            .call(RPCRequestMethod::RunDoctest, Some(params))
            .await?;
        Ok(convert_wire_result(test.clone(), wire))
    }

    /// Verify that the worker process is responsive.
    ///
    /// # Errors
    /// Returns an error if the ping RPC fails or if the worker returns an
    /// unexpected response.
    pub async fn ping(&mut self) -> Result<()> {
        let result: String = self.call(RPCRequestMethod::Ping, None).await?;
        if result == "pong" {
            Ok(())
        } else {
            Err(anyhow!("unexpected ping response: {result}"))
        }
    }

    /// Snapshot the buffered worker stderr and clear the buffer.
    ///
    /// Called on the error path when the worker is about to be
    /// discarded. We kill the child first so the drainer task reaches
    /// EOF, then await the drainer (with a short timeout) so any bytes
    /// still in the kernel pipe at the time of the failure land in
    /// `stderr_buf` before we snapshot. Without this, a python worker
    /// that dies during startup (e.g. `ModuleNotFoundError: tryke`)
    /// can lose its traceback to a race between the RPC's
    /// `Broken pipe` and the drainer task being scheduled.
    ///
    /// # Panics
    /// Panics only if the stderr-drainer task panicked while holding the
    /// internal mutex (poisoning it). That task does no fallible work.
    pub async fn drain_stderr(&mut self) -> String {
        // Worker is about to be discarded; killing the child closes its
        // stderr pipe which gives the drainer its EOF. Idempotent on a
        // process that has already exited.
        let _ = self.child.start_kill();
        if let Some(handle) = self.stderr_drainer.take() {
            let _ = tokio::time::timeout(Duration::from_millis(500), handle).await;
        }
        let bytes: Vec<u8> = {
            let mut g = self
                .stderr_buf
                .lock()
                .expect("stderr buffer mutex poisoned");
            std::mem::take(&mut *g).into_iter().collect()
        };
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
        // Killing the child closes stderr; the drainer will see EOF and
        // exit on its own — but `abort()` is the explicit, immediate
        // signal that we're done with it, and prevents a leftover task
        // from briefly holding the stderr FD + `stderr_buf` Arc past
        // the worker's lifetime.
        if let Some(handle) = self.stderr_drainer.take() {
            handle.abort();
        }
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        // Safety net: ensure the child process is killed when the worker is
        // dropped (e.g. on the error-respawn path in pool.rs). start_kill() is
        // the synchronous variant — safe to call on already-dead processes.
        let _ = self.child.start_kill();
        // Dropping a Tokio JoinHandle detaches the task, so abort it
        // explicitly to avoid an orphan drainer outliving the worker on
        // respawn paths (the drainer holds stderr_buf + the stderr FD).
        if let Some(handle) = self.stderr_drainer.take() {
            handle.abort();
        }
    }
}

/// Spawn a tokio task that continuously reads `stderr` into `buf` until
/// EOF or a read error, capping the buffer at `STDERR_RETAIN_BYTES`.
///
/// Returns an error if no Tokio runtime is currently entered, rather than
/// panicking the way `tokio::spawn` would. This keeps the synchronous
/// `WorkerProcess::spawn` API safe to call from any context — callers in
/// non-async code receive a structured error instead of a panic.
fn spawn_stderr_drainer(
    stderr: tokio::process::ChildStderr,
    buf: Arc<Mutex<VecDeque<u8>>>,
) -> Result<tokio::task::JoinHandle<()>> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|e| anyhow!("WorkerProcess::spawn requires an active tokio runtime: {e}"))?;
    Ok(handle.spawn(async move {
        let mut reader = stderr;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => append_stderr(&buf, &chunk[..n]),
                Err(e) => {
                    trace!("stderr drainer: read error: {e}");
                    break;
                }
            }
        }
    }))
}

/// Append `data` to `buf`, capping it at `STDERR_RETAIN_BYTES` by
/// dropping the oldest bytes. `VecDeque::drain` removes from the front
/// in `O(excess)` time, making steady-state appends `O(data.len())`
/// rather than `O(STDERR_RETAIN_BYTES)` as a contiguous `Vec` would require.
fn append_stderr(buf: &Mutex<VecDeque<u8>>, data: &[u8]) {
    let mut g = buf.lock().expect("stderr buffer mutex poisoned");
    if data.len() >= STDERR_RETAIN_BYTES {
        // A single chunk already fills the cap; keep only its tail.
        g.clear();
        g.extend(data[data.len() - STDERR_RETAIN_BYTES..].iter().copied());
        return;
    }
    let total = g.len() + data.len();
    if total > STDERR_RETAIN_BYTES {
        let excess = total - STDERR_RETAIN_BYTES;
        g.drain(..excess);
    }
    g.extend(data.iter().copied());
}

/// Translate a resolved log level into the value placed on the spawned
/// worker's `TRYKE_LOG` env var, if any.
///
/// `Off` returns `None` so the env var stays unset and the python
/// worker's `_configure_logging_from_env` no-ops. Anything else returns
/// the lowercase level name (`debug`, `info`, ...) which the worker's
/// `logging.getLevelName` understands once uppercased.
fn worker_log_env_value(log_level: log::LevelFilter) -> Option<String> {
    if log_level == log::LevelFilter::Off {
        return None;
    }
    Some(log_level.as_str().to_ascii_lowercase())
}

fn build_pythonpath(extra: &[&Path]) -> String {
    let existing = std::env::var("PYTHONPATH").unwrap_or_default();
    let mut parts: Vec<String> = extra
        .iter()
        .map(|p| {
            let s = p
                .canonicalize()
                .unwrap_or_else(|_| p.to_path_buf())
                .to_string_lossy()
                .into_owned();
            // std::fs::canonicalize on windows produces \\?\ extended-length
            // paths that python doesn't understand
            #[cfg(windows)]
            let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
            s
        })
        .collect();
    if !existing.is_empty() {
        parts.push(existing);
    }
    let sep = if cfg!(windows) { ";" } else { ":" };
    parts.join(sep)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tryke_types::{AssertionWire, ExpectedAssertion, TestOutcome};

    use super::*;

    fn make_test_item() -> TestItem {
        TestItem {
            name: "test_add".into(),
            module_path: "tests.test_math".into(),
            file_path: Some(PathBuf::from("tests/test_math.py")),
            line_number: Some(5),
            display_name: None,
            expected_assertions: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn worker_log_env_value_off_returns_none() {
        // `Off` means: don't set TRYKE_LOG on the child env, preserving
        // the python worker's "no chatter unless asked" default.
        assert_eq!(worker_log_env_value(log::LevelFilter::Off), None);
    }

    #[test]
    fn worker_log_env_value_lowercases_level_name() {
        // The python worker uppercases before passing to
        // `logging.getLevelName`, so case actually doesn't matter, but
        // shipping lowercase keeps env values consistent with how rust
        // log levels render and avoids a 50/50 stylistic decision.
        assert_eq!(
            worker_log_env_value(log::LevelFilter::Info).as_deref(),
            Some("info"),
        );
        assert_eq!(
            worker_log_env_value(log::LevelFilter::Debug).as_deref(),
            Some("debug"),
        );
        assert_eq!(
            worker_log_env_value(log::LevelFilter::Warn).as_deref(),
            Some("warn"),
        );
    }

    #[test]
    fn convert_result_passed() {
        let test = make_test_item();
        let wire = RunTestResultWire::Passed {
            duration_ms: 10,
            stdout: "out".into(),
            stderr: "err".into(),
        };
        let result = convert_wire_result(test, wire);
        assert!(matches!(result.outcome, TestOutcome::Passed));
        assert_eq!(result.duration, Duration::from_millis(10));
        assert_eq!(result.stdout, "out");
        assert_eq!(result.stderr, "err");
    }

    #[test]
    fn convert_result_failed() {
        let test = make_test_item();
        let wire = RunTestResultWire::Failed {
            duration_ms: 5,
            message: "expected 1 got 2".into(),
            traceback: None,
            assertions: vec![],
            executed_lines: vec![],
            stdout: String::new(),
            stderr: String::new(),
        };
        let result = convert_wire_result(test, wire);
        assert!(matches!(
            result.outcome,
            TestOutcome::Failed { ref message, .. } if message == "expected 1 got 2"
        ));
    }

    #[test]
    fn convert_result_skipped() {
        let test = make_test_item();
        let wire = RunTestResultWire::Skipped {
            duration_ms: 0,
            reason: Some("not ready".into()),
            stdout: String::new(),
            stderr: String::new(),
        };
        let result = convert_wire_result(test, wire);
        assert!(matches!(
            result.outcome,
            TestOutcome::Skipped { reason: Some(ref r) } if r == "not ready"
        ));
    }

    #[test]
    fn build_pythonpath_joins_paths() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = build_pythonpath(&[dir_a.path()]);
        let b = build_pythonpath(&[dir_b.path()]);
        let result = build_pythonpath(&[dir_a.path(), dir_b.path()]);
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert_eq!(result, format!("{a}{sep}{b}"));
    }

    #[test]
    fn convert_result_uses_discovered_multiline_assertion_source() {
        let expression =
            "t.expect(\n    expr=actual,\n    name=\"actual should be one\",\n).to_equal(other=1)";
        let test = TestItem {
            expected_assertions: vec![ExpectedAssertion {
                subject: "actual".into(),
                matcher: "to_equal".into(),
                negated: false,
                args: vec!["other=1".into()],
                line: 7,
                label: Some("actual should be one".into()),
                end_line: 10,
                start_column: Some(4),
                end_column: Some(18),
                expression: expression.into(),
                subject_span: expression
                    .find("actual")
                    .map(|offset| (offset, "actual".len())),
                expected_arg_span: expression
                    .find("other=1")
                    .map(|offset| (offset, "other=1".len())),
                expected_arg_value: Some("1".into()),
            }],
            ..Default::default()
        };
        let result = convert_wire_result(
            test,
            RunTestResultWire::Failed {
                duration_ms: 1,
                message: "assertion failed".into(),
                traceback: None,
                assertions: vec![AssertionWire {
                    expression: ".to_equal(other=1)".into(),
                    expected: "1".into(),
                    received: "0".into(),
                    line: 10,
                    column: Some(6),
                    file: Some("tests/test_multiline.py".into()),
                }],
                executed_lines: vec![10],
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let TestOutcome::Failed {
            assertions,
            executed_lines,
            ..
        } = result.outcome
        else {
            panic!("expected failed outcome");
        };
        let assertion = &assertions[0];
        assert_eq!(assertion.expression, expression);
        assert_eq!(assertion.line, 7);
        assert_eq!(assertion.span_offset, expression.find("actual").unwrap());
        assert_eq!(
            assertion.expected_arg_span,
            expression.find("other=1").map(|offset| (offset, 7))
        );
        assert_eq!(executed_lines, vec![7]);
    }

    #[test]
    fn convert_result_selects_inner_assertion_by_column() {
        let test = TestItem {
            expected_assertions: vec![
                ExpectedAssertion {
                    subject: "expect(0).to_equal(1)".into(),
                    matcher: "to_be_truthy".into(),
                    negated: false,
                    args: vec![],
                    line: 3,
                    end_line: 3,
                    start_column: Some(4),
                    end_column: Some(45),
                    expression: "expect(expect(0).to_equal(1)).to_be_truthy()".into(),
                    subject_span: Some((7, 21)),
                    expected_arg_span: None,
                    expected_arg_value: None,
                    label: None,
                },
                ExpectedAssertion {
                    subject: "0".into(),
                    matcher: "to_equal".into(),
                    negated: false,
                    args: vec!["1".into()],
                    line: 3,
                    end_line: 3,
                    start_column: Some(11),
                    end_column: Some(32),
                    expression: "expect(0).to_equal(1)".into(),
                    subject_span: Some((7, 1)),
                    expected_arg_span: Some((19, 1)),
                    expected_arg_value: Some("1".into()),
                    label: None,
                },
            ],
            ..Default::default()
        };
        let result = convert_wire_result(
            test,
            RunTestResultWire::Failed {
                duration_ms: 1,
                message: "assertion failed".into(),
                traceback: None,
                assertions: vec![AssertionWire {
                    expression: "expect(expect(0).to_equal(1)).to_be_truthy()".into(),
                    expected: "1".into(),
                    received: "0".into(),
                    line: 3,
                    column: Some(21),
                    file: None,
                }],
                executed_lines: vec![3],
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let TestOutcome::Failed { assertions, .. } = result.outcome else {
            panic!("expected failed outcome");
        };
        assert_eq!(assertions[0].expression, "expect(0).to_equal(1)");
        assert_eq!(assertions[0].span_offset, 7);
        assert_eq!(assertions[0].expected_arg_span, Some((19, 1)));
    }

    #[test]
    fn convert_result_matches_same_line_assertion_by_expected_value_without_column() {
        let second_expression = "expect(b).to_equal(other=2)";
        let test = TestItem {
            expected_assertions: vec![
                ExpectedAssertion {
                    subject: "a".into(),
                    matcher: "to_equal".into(),
                    negated: false,
                    args: vec!["1".into()],
                    line: 5,
                    end_line: 5,
                    expression: "expect(a).to_equal(1)".into(),
                    subject_span: Some((7, 1)),
                    expected_arg_span: Some((19, 1)),
                    label: None,
                    ..Default::default()
                },
                ExpectedAssertion {
                    subject: "b".into(),
                    matcher: "to_equal".into(),
                    negated: false,
                    args: vec!["other=2".into()],
                    line: 5,
                    end_line: 5,
                    expression: second_expression.into(),
                    subject_span: Some((7, 1)),
                    expected_arg_span: second_expression
                        .find("other=2")
                        .map(|offset| (offset, "other=2".len())),
                    label: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let result = convert_wire_result(
            test,
            RunTestResultWire::Failed {
                duration_ms: 1,
                message: "assertion failed".into(),
                traceback: None,
                assertions: vec![AssertionWire {
                    expression: "expect(a).to_equal(1); expect(b).to_equal(other=2)".into(),
                    expected: "2".into(),
                    received: "0".into(),
                    line: 5,
                    column: None,
                    file: None,
                }],
                executed_lines: vec![5],
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let TestOutcome::Failed { assertions, .. } = result.outcome else {
            panic!("expected failed outcome");
        };
        assert_eq!(assertions[0].expression, second_expression);
        assert_eq!(
            assertions[0].expected_arg_span,
            second_expression
                .find("other=2")
                .map(|offset| (offset, "other=2".len()))
        );
    }

    #[test]
    fn convert_result_keeps_runtime_expression_for_ambiguous_same_line_without_column() {
        let runtime_expression = "expect(a).to_equal(1); expect(b).to_equal(1)";
        let test = TestItem {
            expected_assertions: vec![
                ExpectedAssertion {
                    subject: "a".into(),
                    matcher: "to_equal".into(),
                    negated: false,
                    args: vec!["1".into()],
                    line: 5,
                    end_line: 5,
                    expression: "expect(a).to_equal(1)".into(),
                    subject_span: Some((7, 1)),
                    label: None,
                    ..Default::default()
                },
                ExpectedAssertion {
                    subject: "b".into(),
                    matcher: "to_equal".into(),
                    negated: false,
                    args: vec!["1".into()],
                    line: 5,
                    end_line: 5,
                    expression: "expect(b).to_equal(1)".into(),
                    subject_span: Some((7, 1)),
                    label: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let result = convert_wire_result(
            test,
            RunTestResultWire::Failed {
                duration_ms: 1,
                message: "assertion failed".into(),
                traceback: None,
                assertions: vec![AssertionWire {
                    expression: runtime_expression.into(),
                    expected: "1".into(),
                    received: "0".into(),
                    line: 5,
                    column: None,
                    file: None,
                }],
                executed_lines: vec![5],
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let TestOutcome::Failed { assertions, .. } = result.outcome else {
            panic!("expected failed outcome");
        };
        assert_eq!(assertions[0].expression, runtime_expression);
    }

    #[tokio::test]
    async fn drop_kills_child_process() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("failed to spawn sleep");
        let pid = child.id().expect("missing pid");

        // Wrap in a WorkerProcess-like drop: start_kill then drop
        let _ = child.start_kill();
        drop(child);

        // Give the OS a moment to reap the process
        tokio::time::sleep(Duration::from_millis(50)).await;

        // On Unix, sending signal 0 checks if the process exists
        let status = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            status.is_ok_and(|s| !s.success()),
            "child process {pid} should be dead after start_kill + drop"
        );
    }

    #[test]
    fn append_stderr_caps_at_retain_bytes() {
        let buf = Mutex::new(VecDeque::<u8>::new());
        // Fill with a recognisable head sequence, then push past the cap.
        let head = vec![b'A'; STDERR_RETAIN_BYTES];
        append_stderr(&buf, &head);
        let tail = vec![b'B'; 1024];
        append_stderr(&buf, &tail);

        let mut g = buf.lock().unwrap();
        assert_eq!(g.len(), STDERR_RETAIN_BYTES);
        // Oldest A's are dropped; tail B's retained.
        let bytes = g.make_contiguous();
        assert_eq!(&bytes[bytes.len() - tail.len()..], &tail[..]);
        assert_eq!(bytes[0], b'A');
    }

    #[test]
    fn append_stderr_handles_single_oversized_write() {
        let buf = Mutex::new(VecDeque::<u8>::new());
        let big = vec![b'X'; STDERR_RETAIN_BYTES + 4096];
        append_stderr(&buf, &big);
        let mut g = buf.lock().unwrap();
        assert_eq!(g.len(), STDERR_RETAIN_BYTES);
        assert!(g.make_contiguous().iter().all(|b| *b == b'X'));
    }

    /// Reproducer for the "tryke hangs at `finalize_hooks`" bug: when the
    /// worker writes more than the kernel pipe buffer (~64 KiB on Linux,
    /// ~4 KiB on Windows) to stderr without anyone draining it, the
    /// worker's next stderr write blocks and it stops responding to
    /// RPCs. The drainer task spawned in `WorkerProcess::spawn` must
    /// keep the pipe empty so RPC flow is unaffected.
    ///
    /// Test contract is binary: either the drainer prevents the deadlock
    /// and `done` arrives on stdout, or it doesn't. The cap-retention
    /// property is covered by the `append_stderr_*` unit tests above and
    /// is intentionally NOT asserted here — re-asserting it via a second
    /// post-exit polling loop introduced wall-clock flakiness on slow
    /// Windows runners without adding any signal a unit test wouldn't
    /// catch.
    ///
    /// Uses `python3` (already a project dependency) for portability and
    /// routes through the same `spawn_stderr_drainer` helper as
    /// `WorkerProcess::spawn`, so a regression in the production drainer
    /// path would deadlock this test too.
    #[tokio::test]
    async fn worker_continues_after_large_stderr_write() {
        // 256 KiB is comfortably above both Linux's 64 KiB pipe buffer
        // and Windows's 4 KiB default — enough to deadlock without a
        // drainer. Larger values (we used to write 1 MiB) just make the
        // test slower without proving anything additional.
        const STDERR_BYTES: usize = 256 * 1024;
        let mut child = tokio::process::Command::new("python3")
            .args([
                "-c",
                &format!(
                    "import sys; sys.stderr.write('X' * {STDERR_BYTES}); \
                     sys.stderr.flush(); print('done', flush=True)"
                ),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn python3");

        let stderr = child.stderr.take().expect("no stderr");
        let stderr_buf: Arc<Mutex<VecDeque<u8>>> = Arc::new(Mutex::new(VecDeque::new()));
        spawn_stderr_drainer(stderr, Arc::clone(&stderr_buf))
            .expect("drainer requires tokio runtime");

        let mut stdout = BufReader::new(child.stdout.take().expect("no stdout"));
        let mut line = String::new();
        // 30s catches a real deadlock (which would never finish) while
        // tolerating Windows python startup + scheduler jitter. Past 5s
        // budgets occasionally tripped on Windows × 3.14 even though no
        // deadlock occurred.
        tokio::time::timeout(Duration::from_secs(30), stdout.read_line(&mut line))
            .await
            .expect("stdout read timed out — drainer deadlocked")
            .expect("stdout read errored");
        assert_eq!(line.trim(), "done");

        let _ = child.wait().await;
    }
}
