use std::io;

use tryke_types::{DiscoveryError, RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;
use crate::diagnostic::render_assertions_plain;

pub struct LlmReporter<W: io::Write = io::Stdout> {
    writer: W,
}

impl LlmReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
        }
    }
}

impl Default for LlmReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: io::Write> LlmReporter<W> {
    pub fn with_writer(writer: W) -> Self {
        Self { writer }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }
}

fn format_duration(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.2}ms")
    } else {
        format!("{:.2}s", d.as_secs_f64())
    }
}

fn write_location<W: io::Write>(writer: &mut W, result: &TestResult) {
    if let Some(path) = &result.test.file_path {
        if let Some(line) = result.test.line_number {
            let _ = write!(writer, " ({path}:{line})", path = path.display());
        } else {
            let _ = write!(writer, " ({})", path.display());
        }
    }
}

impl<W: io::Write> Reporter for LlmReporter<W> {
    fn on_run_start(&mut self, _tests: &[TestItem]) {}

    fn on_test_complete(&mut self, result: &TestResult) {
        let display = result
            .test
            .display_name
            .as_deref()
            .unwrap_or(&result.test.name);

        match &result.outcome {
            TestOutcome::Passed
            | TestOutcome::Skipped { .. }
            | TestOutcome::XFailed { .. }
            | TestOutcome::Todo { .. } => {}
            TestOutcome::Failed {
                message,
                traceback,
                assertions,
            } => {
                let _ = write!(self.writer, "FAIL {display}");
                write_location(&mut self.writer, result);
                let _ = writeln!(self.writer);

                if !assertions.is_empty() {
                    let test_file = result
                        .test
                        .file_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned());
                    let mut buf = String::new();
                    render_assertions_plain(test_file.as_deref(), assertions, &mut buf);
                    let _ = write!(self.writer, "{buf}");
                } else if !message.is_empty() {
                    let _ = writeln!(self.writer, "  {message}");
                }

                if let Some(tb) = traceback {
                    let _ = writeln!(self.writer, "  Traceback:");
                    for line in tb.lines() {
                        let _ = writeln!(self.writer, "    {line}");
                    }
                }

                if !result.stdout.is_empty() {
                    let _ = writeln!(self.writer, "  [stdout]");
                    for line in result.stdout.lines() {
                        let _ = writeln!(self.writer, "    {line}");
                    }
                }
                if !result.stderr.is_empty() {
                    let _ = writeln!(self.writer, "  [stderr]");
                    for line in result.stderr.lines() {
                        let _ = writeln!(self.writer, "    {line}");
                    }
                }
            }
            TestOutcome::XPassed => {
                let _ = write!(self.writer, "XPASS {display}");
                write_location(&mut self.writer, result);
                let _ = writeln!(self.writer);
                let _ = writeln!(self.writer, "  unexpected pass");
            }
            TestOutcome::Error { message } => {
                let _ = write!(self.writer, "ERROR {display}");
                write_location(&mut self.writer, result);
                let _ = writeln!(self.writer);
                let _ = writeln!(self.writer, "  {message}");

                if !result.stderr.is_empty() {
                    let _ = writeln!(self.writer, "  [stderr]");
                    for line in result.stderr.lines() {
                        let _ = writeln!(self.writer, "    {line}");
                    }
                }
            }
        }
    }

    fn on_run_complete(&mut self, summary: &RunSummary) {
        let mut parts = Vec::new();
        if summary.passed > 0 {
            parts.push(format!("{} passed", summary.passed));
        }
        if summary.failed > 0 {
            parts.push(format!("{} failed", summary.failed));
        }
        if summary.errors > 0 {
            parts.push(format!("{} error", summary.errors));
        }
        if summary.skipped > 0 {
            parts.push(format!("{} skipped", summary.skipped));
        }
        if summary.xfailed > 0 {
            parts.push(format!("{} xfailed", summary.xfailed));
        }
        if summary.todo > 0 {
            parts.push(format!("{} todo", summary.todo));
        }
        if parts.is_empty() {
            parts.push("0 passed".into());
        }
        let _ = writeln!(
            self.writer,
            "{} [{}]",
            parts.join(", "),
            format_duration(summary.duration)
        );
    }

    fn on_collect_complete(&mut self, tests: &[TestItem]) {
        let _ = writeln!(self.writer, "{} tests collected.", tests.len());
    }

    fn on_discovery_error(&mut self, error: &DiscoveryError) {
        let _ = writeln!(
            self.writer,
            "DISCOVERY ERROR: {}: {}",
            error.file_path.display(),
            error.message
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use tryke_types::{Assertion, TestOutcome};

    use super::*;

    fn reporter() -> LlmReporter<Vec<u8>> {
        LlmReporter::with_writer(Vec::new())
    }

    fn output(r: &LlmReporter<Vec<u8>>) -> String {
        String::from_utf8_lossy(&r.writer).into_owned()
    }

    fn test_item(name: &str) -> TestItem {
        TestItem {
            name: name.into(),
            module_path: "tests.mod".into(),
            ..Default::default()
        }
    }

    fn test_item_with_file(name: &str, file: &str, line: u32) -> TestItem {
        TestItem {
            name: name.into(),
            module_path: "tests.mod".into(),
            file_path: Some(PathBuf::from(file)),
            line_number: Some(line),
            ..Default::default()
        }
    }

    #[test]
    fn passed_produces_no_output() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item("test_add"),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        assert!(output(&r).is_empty());
    }

    #[test]
    fn skipped_produces_no_output() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item("test_skip"),
            outcome: TestOutcome::Skipped {
                reason: Some("not ready".into()),
            },
            duration: Duration::from_millis(0),
            stdout: String::new(),
            stderr: String::new(),
        });
        assert!(output(&r).is_empty());
    }

    #[test]
    fn failed_shows_name_and_location() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item_with_file("test_sub", "tests/math.py", 15),
            outcome: TestOutcome::Failed {
                message: "bad".into(),
                traceback: None,
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(&r);
        assert!(out.contains("FAIL test_sub (tests/math.py:15)"));
        assert!(out.contains("bad"));
    }

    #[test]
    fn failed_with_assertions_plain_text() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item_with_file("test_add", "tests/math.py", 10),
            outcome: TestOutcome::Failed {
                message: "assertion failed".into(),
                traceback: None,
                assertions: vec![Assertion {
                    expression: "expect(a).to_equal(2)".into(),
                    file: None,
                    line: 10,
                    span_offset: 7,
                    span_length: 1,
                    expected: "2".into(),
                    received: "3".into(),
                    expected_arg_span: Some((19, 1)),
                }],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(&r);
        assert!(out.contains("FAIL test_add"));
        assert!(out.contains("received 3"));
        assert!(out.contains("expected 2"));
        assert!(out.contains("1/1 assertions failed"));
    }

    #[test]
    fn failed_with_traceback_shows_full_traceback() {
        let traceback = "File \"tests/test_math.py\", line 10, in test_div\n  result = divide(1, 0)\nFile \"math_utils.py\", line 3, in divide\n  return a / b\nZeroDivisionError: division by zero";
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item_with_file("test_div", "tests/test_math.py", 10),
            outcome: TestOutcome::Failed {
                message: "division by zero".into(),
                traceback: Some(traceback.into()),
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(&r);
        assert!(out.contains("Traceback:"));
        assert!(out.contains("File \"tests/test_math.py\", line 10, in test_div"));
        assert!(out.contains("result = divide(1, 0)"));
        assert!(out.contains("File \"math_utils.py\", line 3, in divide"));
        assert!(out.contains("ZeroDivisionError: division by zero"));
    }

    #[test]
    fn failed_with_captured_output() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item("test_out"),
            outcome: TestOutcome::Failed {
                message: "fail".into(),
                traceback: None,
                assertions: vec![],
            },
            duration: Duration::from_millis(1),
            stdout: "debug output here".into(),
            stderr: "warning here".into(),
        });
        let out = output(&r);
        assert!(out.contains("[stdout]"));
        assert!(out.contains("debug output here"));
        assert!(out.contains("[stderr]"));
        assert!(out.contains("warning here"));
    }

    #[test]
    fn error_shows_name_and_message() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item_with_file("test_broken", "tests/broken.py", 1),
            outcome: TestOutcome::Error {
                message: "worker spawn failed: No such file".into(),
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(&r);
        assert!(out.contains("ERROR test_broken (tests/broken.py:1)"));
        assert!(out.contains("worker spawn failed: No such file"));
    }

    #[test]
    fn summary_all_pass() {
        let mut r = reporter();
        r.on_run_complete(&RunSummary {
            passed: 47,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(35),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
        });
        let out = output(&r);
        assert_eq!(out.trim(), "47 passed [35.00ms]");
    }

    #[test]
    fn summary_mixed() {
        let mut r = reporter();
        r.on_run_complete(&RunSummary {
            passed: 47,
            failed: 2,
            skipped: 3,
            errors: 1,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(35),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
        });
        let out = output(&r);
        assert_eq!(
            out.trim(),
            "47 passed, 2 failed, 1 error, 3 skipped [35.00ms]"
        );
    }

    #[test]
    fn no_ansi_codes_in_output() {
        let mut r = reporter();
        r.on_run_start(&[test_item("t")]);
        r.on_test_complete(&TestResult {
            test: test_item_with_file("test_fail", "tests/a.py", 1),
            outcome: TestOutcome::Failed {
                message: "bad".into(),
                traceback: None,
                assertions: vec![],
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        r.on_run_complete(&RunSummary {
            passed: 0,
            failed: 1,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(1),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
        });
        let out = output(&r);
        assert!(
            !out.contains("\x1b["),
            "output must not contain ANSI escapes"
        );
    }

    #[test]
    fn collect_complete_shows_count() {
        let mut r = reporter();
        let tests = vec![test_item("a"), test_item("b"), test_item("c")];
        r.on_collect_complete(&tests);
        assert_eq!(output(&r).trim(), "3 tests collected.");
    }

    #[test]
    fn discovery_error_plain_text() {
        let mut r = reporter();
        r.on_discovery_error(&DiscoveryError {
            file_path: PathBuf::from("tests/broken.py"),
            message: "syntax error on line 5".into(),
            line_number: Some(5),
        });
        let out = output(&r);
        assert_eq!(
            out.trim(),
            "DISCOVERY ERROR: tests/broken.py: syntax error on line 5"
        );
    }

    #[test]
    fn failed_without_file_path() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item("test_no_file"),
            outcome: TestOutcome::Failed {
                message: "oops".into(),
                traceback: None,
                assertions: vec![],
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(&r);
        assert!(out.starts_with("FAIL test_no_file\n"));
        assert!(!out.contains('('));
    }

    #[test]
    fn full_lifecycle() {
        let mut r = reporter();
        let items = vec![
            test_item_with_file("test_pass", "tests/a.py", 1),
            test_item_with_file("test_fail", "tests/a.py", 5),
            test_item("test_skip"),
        ];

        r.on_run_start(&items);
        r.on_test_complete(&TestResult {
            test: items[0].clone(),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(10),
            stdout: String::new(),
            stderr: String::new(),
        });
        r.on_test_complete(&TestResult {
            test: items[1].clone(),
            outcome: TestOutcome::Failed {
                message: "bad".into(),
                traceback: None,
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });
        r.on_test_complete(&TestResult {
            test: items[2].clone(),
            outcome: TestOutcome::Skipped { reason: None },
            duration: Duration::from_millis(0),
            stdout: String::new(),
            stderr: String::new(),
        });
        r.on_run_complete(&RunSummary {
            passed: 1,
            failed: 1,
            skipped: 1,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(15),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
        });

        let out = output(&r);
        // No output for passed/skipped tests
        assert!(!out.contains("test_pass"));
        assert!(!out.contains("test_skip"));
        // Failure shown
        assert!(out.contains("FAIL test_fail (tests/a.py:5)"));
        // Summary
        assert!(out.contains("1 passed, 1 failed, 1 skipped"));
    }
}
