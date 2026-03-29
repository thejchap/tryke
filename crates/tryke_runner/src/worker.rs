use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow};
use log::{debug, trace};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tryke_types::{Assertion, ExpectedAssertion, TestItem, TestOutcome, TestResult};

use crate::protocol::{
    AssertionWire, ReloadParams, RpcRequest, RpcResponse, RunDoctestParams, RunTestParams,
    RunTestResultWire,
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
    pub fn spawn(python_bin: &str, python_path: &[&Path], root: &Path) -> Result<Self> {
        debug!("spawning worker: {python_bin} -m tryke.worker");
        let pythonpath = build_pythonpath(python_path);
        let mut child = Command::new(python_bin)
            .args(["-m", "tryke.worker"])
            .env("PYTHONPATH", &pythonpath)
            .current_dir(root)
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
        trace!("worker rpc -> {}", line.trim());
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        trace!("worker rpc: waiting for response");
        let mut resp_line = String::new();
        let n = self.stdout.read_line(&mut resp_line).await?;
        if n == 0 {
            trace!("worker rpc: stdout EOF");
            return Err(anyhow!("worker process closed stdout"));
        }
        trace!("worker rpc <- {}", resp_line.trim());
        let trimmed = resp_line.trim();
        let resp: RpcResponse = serde_json::from_str(trimmed).map_err(|e| {
            if trimmed.is_empty() {
                anyhow!(
                    "expected JSON-RPC response from worker but got an empty line \
                     (a library may have written to stdout during import)"
                )
            } else {
                let preview = if trimmed.len() > 200 {
                    format!("{}...", &trimmed[..200])
                } else {
                    trimmed.to_string()
                };
                anyhow!(
                    "expected JSON-RPC response from worker but got: {preview}\n\
                     parse error: {e}"
                )
            }
        })?;
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
        if let Some(object_path) = &test.doctest_object {
            return self.run_doctest(test, object_path).await;
        }
        let params = serde_json::to_value(RunTestParams {
            module: test.module_path.clone(),
            function: test.name.clone(),
            xfail: test.xfail.clone(),
        })?;
        let wire: RunTestResultWire = self.call("run_test", Some(params)).await?;
        Ok(convert_result(test.clone(), wire))
    }

    async fn run_doctest(&mut self, test: &TestItem, object_path: &str) -> Result<TestResult> {
        let params = serde_json::to_value(RunDoctestParams {
            module: test.module_path.clone(),
            object_path: object_path.to_owned(),
        })?;
        let wire: RunTestResultWire = self.call("run_doctest", Some(params)).await?;
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
            trace!("drain_stderr: timed out");
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    pub async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        // Safety net: ensure the child process is killed when the worker is
        // dropped (e.g. on the error-respawn path in pool.rs). start_kill() is
        // the synchronous variant — safe to call on already-dead processes.
        let _ = self.child.start_kill();
    }
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

fn convert_assertion(wire: AssertionWire, expected_arg_span: Option<(usize, usize)>) -> Assertion {
    let (span_offset, span_length) = compute_subject_span(&wire.expression);
    // Make absolute paths relative to cwd so diagnostics show short paths.
    let file = wire.file.map(|f| {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| {
                Path::new(&f)
                    .strip_prefix(&cwd)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or(f)
    });
    Assertion {
        expression: wire.expression,
        file,
        line: wire.line as usize,
        span_offset,
        span_length,
        expected: wire.expected,
        received: wire.received,
        expected_arg_span,
    }
}

/// Find the byte span of the first matcher argument inside `expression`,
/// guided by AST metadata from `ExpectedAssertion`. Returns `None` for
/// no-arg matchers like `to_be_falsy()`.
fn find_arg_span(expression: &str, ea: &ExpectedAssertion) -> Option<(usize, usize)> {
    let arg = ea.args.first()?;
    if arg.is_empty() {
        return None;
    }
    // Find `matcher(` after the subject portion
    let matcher_pat = format!("{}(", ea.matcher);
    let matcher_pos = expression.find(&matcher_pat)?;
    let content_start = matcher_pos + matcher_pat.len();
    // Find the arg text right at or after the opening paren
    let remaining = expression.get(content_start..)?;
    let arg_offset_in_remaining = remaining.find(arg.as_str())?;
    let offset = content_start + arg_offset_in_remaining;
    Some((offset, arg.len()))
}

/// Find the first argument inside `expect(subject, ...)` by matching parens.
/// Stops at the first top-level comma so that optional arguments like the
/// label in `expect(val, "label")` are excluded from the span.
fn compute_subject_span(expression: &str) -> (usize, usize) {
    let Some(pos) = expression.find("expect(") else {
        return (0, expression.len().max(1));
    };
    let start = pos + 7; // len("expect(")
    let mut depth: u32 = 1;
    for (i, ch) in expression[start..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 1 => return (start, i.max(1)),
            ')' => depth -= 1,
            ',' if depth == 1 => {
                let len = expression[start..start + i].trim_end().len();
                return (start, len.max(1));
            }
            _ => {}
        }
    }
    (start, expression.len().saturating_sub(start).max(1))
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
        } => {
            let expected_by_line: std::collections::HashMap<u32, &ExpectedAssertion> = test
                .expected_assertions
                .iter()
                .map(|ea| (ea.line, ea))
                .collect();

            let assertions = assertions
                .into_iter()
                .map(|wire| {
                    let expected_arg_span = expected_by_line
                        .get(&wire.line)
                        .and_then(|ea| find_arg_span(&wire.expression, ea));
                    convert_assertion(wire, expected_arg_span)
                })
                .collect();

            TestResult {
                test,
                outcome: TestOutcome::Failed {
                    message,
                    traceback,
                    assertions,
                },
                duration: Duration::from_millis(duration_ms),
                stdout,
                stderr,
            }
        }
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
        RunTestResultWire::XFailed {
            duration_ms,
            reason,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::XFailed { reason },
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::XPassed {
            duration_ms,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::XPassed,
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::Todo {
            duration_ms,
            description,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Todo { description },
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
            ..Default::default()
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
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = build_pythonpath(&[dir_a.path()]);
        let b = build_pythonpath(&[dir_b.path()]);
        let result = build_pythonpath(&[dir_a.path(), dir_b.path()]);
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert_eq!(result, format!("{a}{sep}{b}"));
    }

    #[test]
    fn convert_assertion_maps_wire_fields() {
        let wire = AssertionWire {
            expression: "expect(x).to_equal(2)".into(),
            expected: "2".into(),
            received: "3".into(),
            line: 10,
            file: Some("tests/test_math.py".into()),
        };
        let a = convert_assertion(wire, Some((19, 1)));
        assert_eq!(a.expression, "expect(x).to_equal(2)");
        assert_eq!(a.expected, "2");
        assert_eq!(a.received, "3");
        assert_eq!(a.line, 10);
        assert_eq!(a.file.as_deref(), Some("tests/test_math.py"));
        // "expect(" is 7 chars, subject "x" starts at 7, length 1
        assert_eq!(a.span_offset, 7);
        assert_eq!(a.span_length, 1);
        assert_eq!(a.expected_arg_span, Some((19, 1)));
    }

    #[test]
    fn compute_subject_span_simple() {
        let (off, len) = compute_subject_span("expect(x).to_equal(2)");
        assert_eq!(off, 7);
        assert_eq!(len, 1);
    }

    #[test]
    fn compute_subject_span_nested_parens() {
        let (off, len) = compute_subject_span("expect(foo(bar(1))).to_be_truthy()");
        assert_eq!(off, 7);
        assert_eq!(len, 11); // "foo(bar(1))"
    }

    #[test]
    fn compute_subject_span_no_expect() {
        let (off, len) = compute_subject_span("assert x == 1");
        assert_eq!(off, 0);
        assert_eq!(len, 13);
    }

    #[test]
    fn compute_subject_span_stops_at_comma() {
        let (off, len) = compute_subject_span("expect(val, \"label\")");
        assert_eq!(off, 7);
        assert_eq!(len, 3); // "val"
    }

    #[test]
    fn compute_subject_span_nested_parens_with_label() {
        let (off, len) = compute_subject_span("expect(foo(a, b), \"label\")");
        assert_eq!(off, 7);
        assert_eq!(len, 9); // "foo(a, b)"
    }

    #[test]
    fn compute_subject_span_whitespace_before_comma() {
        let (off, len) = compute_subject_span("expect(val  , \"label\")");
        assert_eq!(off, 7);
        assert_eq!(len, 3); // "val" (trailing whitespace trimmed)
    }

    fn make_expected(matcher: &str, args: &[&str], line: u32) -> ExpectedAssertion {
        ExpectedAssertion {
            subject: "x".into(),
            matcher: matcher.into(),
            negated: false,
            args: args.iter().map(|s| (*s).to_string()).collect(),
            line,
            label: None,
        }
    }

    #[test]
    fn find_arg_span_matcher_with_arg() {
        let ea = make_expected("to_equal", &["2"], 10);
        let span = find_arg_span("expect(x).to_equal(2)", &ea);
        assert_eq!(span, Some((19, 1)));
    }

    #[test]
    fn find_arg_span_no_arg_matcher() {
        let ea = make_expected("to_be_falsy", &[], 10);
        let span = find_arg_span("expect(val).to_be_falsy()", &ea);
        assert_eq!(span, None);
    }

    #[test]
    fn find_arg_span_complex_expression() {
        let ea = ExpectedAssertion {
            subject: "foo(bar(1))".into(),
            matcher: "to_equal".into(),
            negated: false,
            args: vec!["42".into()],
            line: 10,
            label: None,
        };
        let span = find_arg_span("expect(foo(bar(1))).to_equal(42)", &ea);
        assert_eq!(span, Some((29, 2)));
    }

    #[test]
    fn find_arg_span_string_arg() {
        let ea = make_expected("to_equal", &["\"hello\""], 10);
        let span = find_arg_span("expect(x).to_equal(\"hello\")", &ea);
        assert_eq!(span, Some((19, 7)));
    }

    #[test]
    fn find_arg_span_matcher_not_found() {
        let ea = make_expected("to_contain", &["5"], 10);
        let span = find_arg_span("expect(x).to_equal(2)", &ea);
        assert_eq!(span, None);
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
}
