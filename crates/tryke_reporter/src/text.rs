use std::io;

use tryke_types::{RunSummary, TestCase, TestOutcome, TestResult};

use crate::Reporter;
use crate::diagnostic::render_assertions;

pub struct TextReporter<W: io::Write = io::Stdout> {
    writer: W,
}

impl TextReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
        }
    }
}

impl Default for TextReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: io::Write> TextReporter<W> {
    pub fn with_writer(writer: W) -> Self {
        Self { writer }
    }
}

impl<W: io::Write> Reporter for TextReporter<W> {
    fn on_run_start(&mut self, tests: &[TestCase]) {
        let _ = writeln!(self.writer, "running {} tests", tests.len());
    }

    fn on_test_complete(&mut self, result: &TestResult) {
        let status = match &result.outcome {
            TestOutcome::Passed => "ok",
            TestOutcome::Failed { .. } => "FAILED",
            TestOutcome::Skipped { .. } => "skipped",
        };
        let _ = writeln!(
            self.writer,
            "  {}::{} ... {} ({:.3}s)",
            result.test.module,
            result.test.name,
            status,
            result.duration.as_secs_f64()
        );

        if let TestOutcome::Failed { assertions, .. } = &result.outcome
            && !assertions.is_empty()
        {
            let mut buf = String::new();
            render_assertions(result.test.file.as_deref(), assertions, &mut buf);
            let _ = write!(self.writer, "{buf}");
        }
    }

    fn on_run_complete(&mut self, summary: &RunSummary) {
        let _ = writeln!(self.writer);
        let _ = writeln!(
            self.writer,
            "test result: {} passed, {} failed, {} skipped; finished in {:.3}s",
            summary.passed,
            summary.failed,
            summary.skipped,
            summary.duration.as_secs_f64()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tryke_types::{Assertion, TestOutcome};

    use super::*;

    fn reporter() -> TextReporter<Vec<u8>> {
        TextReporter::with_writer(Vec::new())
    }

    fn output(reporter: &TextReporter<Vec<u8>>) -> String {
        String::from_utf8_lossy(&reporter.writer).into_owned()
    }

    #[test]
    fn run_start_shows_count() {
        let mut r = reporter();
        let tests = vec![
            TestCase {
                name: "test_a".into(),
                module: "m".into(),
                file: None,
            },
            TestCase {
                name: "test_b".into(),
                module: "m".into(),
                file: None,
            },
        ];

        r.on_run_start(&tests);
        assert_eq!(output(&r), "running 2 tests\n");
    }

    #[test]
    fn test_complete_passed() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestCase {
                name: "test_add".into(),
                module: "math".into(),
                file: None,
            },
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(12),
        });

        assert!(output(&r).contains("math::test_add ... ok"));
    }

    #[test]
    fn test_complete_failed() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestCase {
                name: "test_sub".into(),
                module: "math".into(),
                file: None,
            },
            outcome: TestOutcome::Failed {
                message: "bad".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
        });

        assert!(output(&r).contains("math::test_sub ... FAILED"));
    }

    #[test]
    fn test_complete_skipped() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestCase {
                name: "test_skip".into(),
                module: "misc".into(),
                file: None,
            },
            outcome: TestOutcome::Skipped { reason: None },
            duration: Duration::from_millis(0),
        });

        assert!(output(&r).contains("misc::test_skip ... skipped"));
    }

    #[test]
    fn run_complete_shows_summary() {
        let mut r = reporter();
        r.on_run_complete(&RunSummary {
            passed: 3,
            failed: 1,
            skipped: 2,
            duration: Duration::from_millis(100),
        });

        let out = output(&r);
        assert!(out.contains("3 passed"));
        assert!(out.contains("1 failed"));
        assert!(out.contains("2 skipped"));
    }

    #[test]
    fn full_lifecycle() {
        let mut r = reporter();
        let tests = vec![TestCase {
            name: "test_one".into(),
            module: "mod_a".into(),
            file: None,
        }];

        r.on_run_start(&tests);
        r.on_test_complete(&TestResult {
            test: tests[0].clone(),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(10),
        });
        r.on_run_complete(&RunSummary {
            passed: 1,
            failed: 0,
            skipped: 0,
            duration: Duration::from_millis(10),
        });

        let out = output(&r);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "running 1 tests");
        assert!(lines[1].contains("mod_a::test_one ... ok"));
        // line 2 is blank
        assert!(lines[3].contains("1 passed"));
    }

    #[test]
    fn failed_with_assertions_renders_diagnostics() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestCase {
                name: "test_add".into(),
                module: "math".into(),
                file: Some("tests/math.rs".into()),
            },
            outcome: TestOutcome::Failed {
                message: "assertion failed".into(),
                assertions: vec![Assertion {
                    expression: "assert_eq!(a, 2)".into(),
                    line: 10,
                    span_offset: 14,
                    span_length: 1,
                    expected: "2".into(),
                    received: "3".into(),
                }],
            },
            duration: Duration::from_millis(5),
        });

        let out = output(&r);
        assert!(out.contains("FAILED"));
        assert!(out.contains("assertion failed"));
        assert!(out.contains("expected 2, received 3"));
    }

    #[test]
    fn failed_with_empty_assertions_no_diagnostics() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestCase {
                name: "test_sub".into(),
                module: "math".into(),
                file: None,
            },
            outcome: TestOutcome::Failed {
                message: "bad".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
        });

        let out = output(&r);
        assert!(out.contains("FAILED"));
        // should not contain diagnostic output
        assert!(!out.contains("assertions failed"));
    }
}
