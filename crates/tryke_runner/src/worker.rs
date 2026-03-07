use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow};
use log::debug;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tryke_types::{Assertion, TestItem, TestOutcome, TestResult};

use crate::protocol::{
    AssertionWire, ReloadParams, RpcRequest, RpcResponse, RunTestParams, RunTestResultWire,
};

pub struct WorkerProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: Option<ChildStderr>,
    next_id: u64,
}

impl WorkerProcess {
    #[expect(clippy::missing_errors_doc)]
    pub fn spawn(python_bin: &str, python_path: &[&Path]) -> Result<Self> {
        debug!("spawning worker: {python_bin} -m tryke.worker");
        let pythonpath = build_pythonpath(python_path);
        let mut child = Command::new(python_bin)
            .args(["-m", "tryke.worker"])
            .env("PYTHONPATH", &pythonpath)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let stdin = BufWriter::new(child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?);
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        let stderr = child.stderr.take();
        debug!("worker spawned (pid {:?})", child.id());
        Ok(Self {
            child,
            stdin,
            stdout,
            stderr,
            next_id: 1,
        })
    }

    async fn call<R: for<'de> serde::Deserialize<'de>>(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<R> {
        let id = self.next_id;
        self.next_id += 1;
        let req = RpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        debug!("worker rpc -> {}", line.trim());
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        debug!("worker rpc: waiting for response");
        let mut resp_line = String::new();
        let n = self.stdout.read_line(&mut resp_line).await?;
        if n == 0 {
            debug!("worker rpc: stdout EOF");
            return Err(anyhow!("worker process closed stdout"));
        }
        debug!("worker rpc <- {}", resp_line.trim());
        let resp: RpcResponse = serde_json::from_str(resp_line.trim())?;
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

    #[expect(clippy::missing_errors_doc)]
    pub async fn run_test(&mut self, test: &TestItem) -> Result<TestResult> {
        let params = serde_json::to_value(RunTestParams {
            module: test.module_path.clone(),
            function: test.name.clone(),
        })?;
        let wire: RunTestResultWire = self.call("run_test", Some(params)).await?;
        Ok(convert_result(test.clone(), wire))
    }

    #[expect(clippy::missing_errors_doc)]
    pub async fn reload(&mut self, modules: &[String]) -> Result<()> {
        let params = serde_json::to_value(ReloadParams {
            modules: modules.to_vec(),
        })?;
        self.call::<serde_json::Value>("reload", Some(params))
            .await?;
        Ok(())
    }

    #[expect(clippy::missing_errors_doc)]
    pub async fn ping(&mut self) -> Result<()> {
        let result: String = self.call("ping", None).await?;
        if result == "pong" {
            Ok(())
        } else {
            Err(anyhow!("unexpected ping response: {result}"))
        }
    }

    pub async fn drain_stderr(&mut self) -> String {
        let Some(stderr) = self.stderr.take() else {
            return String::new();
        };
        let mut buf = Vec::new();
        let result = tokio::time::timeout(Duration::from_secs(1), async {
            let mut reader = stderr;
            reader.read_to_end(&mut buf).await
        })
        .await;
        if result.is_err() {
            debug!("drain_stderr: timed out");
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    pub async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
    }
}

fn build_pythonpath(extra: &[&Path]) -> String {
    let existing = std::env::var("PYTHONPATH").unwrap_or_default();
    let mut parts: Vec<String> = extra
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if !existing.is_empty() {
        parts.push(existing);
    }
    let sep = if cfg!(windows) { ";" } else { ":" };
    parts.join(sep)
}

fn convert_assertion(wire: AssertionWire) -> Assertion {
    Assertion {
        expression: format!("{}.{}({})", wire.subject, wire.matcher, wire.actual),
        file: None,
        line: wire.line as usize,
        span_offset: 0,
        span_length: 0,
        expected: String::new(),
        received: wire.actual,
    }
}

fn convert_result(test: TestItem, wire: RunTestResultWire) -> TestResult {
    match wire {
        RunTestResultWire::Passed {
            duration_ms,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::Failed {
            duration_ms,
            message,
            traceback,
            assertions,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Failed {
                message,
                traceback,
                assertions: assertions.into_iter().map(convert_assertion).collect(),
            },
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::Skipped {
            duration_ms,
            reason,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Skipped { reason },
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_test_item() -> TestItem {
        TestItem {
            name: "test_add".into(),
            module_path: "tests.test_math".into(),
            file_path: Some(PathBuf::from("tests/test_math.py")),
            line_number: Some(5),
            display_name: None,
            expected_assertions: vec![],
        }
    }

    #[test]
    fn convert_result_passed() {
        let test = make_test_item();
        let wire = RunTestResultWire::Passed {
            duration_ms: 10,
            stdout: "out".into(),
            stderr: "err".into(),
        };
        let result = convert_result(test, wire);
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
            stdout: String::new(),
            stderr: String::new(),
        };
        let result = convert_result(test, wire);
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
        let result = convert_result(test, wire);
        assert!(matches!(
            result.outcome,
            TestOutcome::Skipped { reason: Some(ref r) } if r == "not ready"
        ));
    }

    #[test]
    fn build_pythonpath_joins_paths() {
        let paths = vec![Path::new("/a"), Path::new("/b")];
        let result = build_pythonpath(&paths);
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert_eq!(result, format!("/a{sep}/b"));
    }

    #[test]
    fn convert_assertion_formats_expression() {
        let wire = AssertionWire {
            subject: "x".into(),
            matcher: "to_equal".into(),
            actual: "2".into(),
            line: 10,
        };
        let a = convert_assertion(wire);
        assert_eq!(a.expression, "x.to_equal(2)");
        assert_eq!(a.received, "2");
        assert_eq!(a.line, 10);
    }
}
