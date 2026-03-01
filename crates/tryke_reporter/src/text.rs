use std::io;
use std::time::Duration;

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

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

fn format_duration(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.2}ms")
    } else {
        format!("{:.2}s", d.as_secs_f64())
    }
}

impl<W: io::Write> Reporter for TextReporter<W> {
    fn on_run_start(&mut self, _tests: &[TestItem]) {
        let _ = writeln!(
            self.writer,
            "{} {}",
            "tryke test".bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        );
        let _ = writeln!(self.writer);
    }

    fn on_test_complete(&mut self, result: &TestResult) {
        match &result.outcome {
            TestOutcome::Passed => {
                let _ = writeln!(
                    self.writer,
                    "{} {} {}",
                    "✓".green(),
                    result.test.name.bold(),
                    format!("[{}]", format_duration(result.duration)).dimmed()
                );
            }
            TestOutcome::Failed { assertions, .. } => {
                let _ = writeln!(
                    self.writer,
                    "{} {} {}",
                    "✗".red(),
                    result.test.name.bold(),
                    format!("[{}]", format_duration(result.duration)).dimmed()
                );

                if !assertions.is_empty() {
                    let test_file = result
                        .test
                        .file_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned());
                    let mut buf = String::new();
                    render_assertions(test_file.as_deref(), assertions, &mut buf);
                    let _ = write!(self.writer, "{buf}");
                }
            }
            TestOutcome::Skipped { .. } => {
                let _ = writeln!(
                    self.writer,
                    "{} {}",
                    "»".yellow().dimmed(),
                    result.test.name.dimmed()
                );
            }
        }
    }

    fn on_run_complete(&mut self, summary: &RunSummary) {
        let _ = writeln!(self.writer);

        let _ = writeln!(
            self.writer,
            " {} {}",
            summary.passed.green(),
            "pass".green()
        );

        if summary.failed > 0 {
            let _ = writeln!(self.writer, " {} {}", summary.failed.red(), "fail".red());
        }

        if summary.skipped > 0 {
            let _ = writeln!(
                self.writer,
                " {} {}",
                summary.skipped.yellow(),
                "skip".yellow()
            );
        }

        let total = summary.passed + summary.failed + summary.skipped;
        let _ = writeln!(
            self.writer,
            "Ran {} tests. [{}]",
            total,
            format_duration(summary.duration)
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
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
    fn run_start_shows_version_header() {
        let mut r = reporter();
        let tests = vec![
            TestItem {
                name: "test_a".into(),
                module_path: "tests.m".into(),
                file_path: None,
                line_number: None,
            },
            TestItem {
                name: "test_b".into(),
                module_path: "tests.m".into(),
                file_path: None,
                line_number: None,
            },
        ];

        r.on_run_start(&tests);
        let out = output(&r);
        assert!(out.contains("tryke test"));
    }

    #[test]
    fn test_complete_passed() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_add".into(),
                module_path: "tests.math".into(),
                file_path: None,
                line_number: None,
            },
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(12),
            stdout: String::new(),
            stderr: String::new(),
        });

        let out = output(&r);
        assert!(out.contains("✓"));
        assert!(out.contains("test_add"));
    }

    #[test]
    fn test_complete_failed() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_sub".into(),
                module_path: "tests.math".into(),
                file_path: None,
                line_number: None,
            },
            outcome: TestOutcome::Failed {
                message: "bad".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });

        let out = output(&r);
        assert!(out.contains("✗"));
        assert!(out.contains("test_sub"));
    }

    #[test]
    fn test_complete_skipped() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_skip".into(),
                module_path: "tests.misc".into(),
                file_path: None,
                line_number: None,
            },
            outcome: TestOutcome::Skipped { reason: None },
            duration: Duration::from_millis(0),
            stdout: String::new(),
            stderr: String::new(),
        });

        let out = output(&r);
        assert!(out.contains("»"));
        assert!(out.contains("test_skip"));
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
        assert!(out.contains('3'));
        assert!(out.contains("pass"));
        assert!(out.contains('1'));
        assert!(out.contains("fail"));
        assert!(out.contains('2'));
        assert!(out.contains("skip"));
        assert!(out.contains("Ran 6 tests"));
    }

    #[test]
    fn run_complete_hides_zero_fail_and_skip() {
        let mut r = reporter();
        r.on_run_complete(&RunSummary {
            passed: 5,
            failed: 0,
            skipped: 0,
            duration: Duration::from_millis(50),
        });

        let out = output(&r);
        assert!(out.contains("pass"));
        assert!(!out.contains("fail"));
        assert!(!out.contains("skip"));
        assert!(out.contains("Ran 5 tests"));
    }

    #[test]
    fn full_lifecycle() {
        let mut r = reporter();
        let tests = vec![TestItem {
            name: "test_one".into(),
            module_path: "tests.mod_a".into(),
            file_path: None,
            line_number: None,
        }];

        r.on_run_start(&tests);
        r.on_test_complete(&TestResult {
            test: tests[0].clone(),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(10),
            stdout: String::new(),
            stderr: String::new(),
        });
        r.on_run_complete(&RunSummary {
            passed: 1,
            failed: 0,
            skipped: 0,
            duration: Duration::from_millis(10),
        });

        let out = output(&r);
        assert!(out.contains("tryke test"));
        assert!(out.contains("✓"));
        assert!(out.contains("test_one"));
        assert!(out.contains("pass"));
        assert!(out.contains("Ran 1 tests"));
    }

    #[test]
    fn failed_with_assertions_renders_diagnostics() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_add".into(),
                module_path: "tests.math".into(),
                file_path: Some(PathBuf::from("tests/math.py")),
                line_number: Some(10),
            },
            outcome: TestOutcome::Failed {
                message: "assertion failed".into(),
                assertions: vec![Assertion {
                    expression: "assert_eq!(a, 2)".into(),
                    file: None,
                    line: 10,
                    span_offset: 14,
                    span_length: 1,
                    expected: "2".into(),
                    received: "3".into(),
                }],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });

        let out = output(&r);
        assert!(out.contains("✗"));
        assert!(out.contains("assertion failed"));
        assert!(out.contains("expected 2, received 3"));
    }

    #[test]
    fn failed_with_empty_assertions_no_diagnostics() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_sub".into(),
                module_path: "tests.math".into(),
                file_path: None,
                line_number: None,
            },
            outcome: TestOutcome::Failed {
                message: "bad".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });

        let out = output(&r);
        assert!(out.contains("✗"));
        assert!(!out.contains("assertions failed"));
    }

    #[test]
    fn format_duration_millis() {
        let d = Duration::from_millis(48);
        assert_eq!(format_duration(d), "48.00ms");
    }

    #[test]
    fn format_duration_seconds() {
        let d = Duration::from_millis(1500);
        assert_eq!(format_duration(d), "1.50s");
    }

    #[test]
    fn format_duration_sub_millis() {
        let d = Duration::from_micros(170);
        assert_eq!(format_duration(d), "0.17ms");
    }
}
