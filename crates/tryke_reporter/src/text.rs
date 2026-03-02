use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;
use crate::diagnostic::render_assertions;

#[derive(Debug, Clone, Copy, Default)]
pub enum Verbosity {
    Quiet,
    #[default]
    Normal,
    Verbose,
}

pub struct TextReporter<W: io::Write = io::Stdout> {
    writer: W,
    current_file: Option<PathBuf>,
    verbosity: Verbosity,
}

impl TextReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
            current_file: None,
            verbosity: Verbosity::Normal,
        }
    }

    #[must_use]
    pub fn with_verbosity(verbosity: Verbosity) -> Self {
        Self {
            writer: io::stdout(),
            current_file: None,
            verbosity,
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
        Self {
            writer,
            current_file: None,
            verbosity: Verbosity::Normal,
        }
    }

    pub fn with_writer_and_verbosity(writer: W, verbosity: Verbosity) -> Self {
        Self {
            writer,
            current_file: None,
            verbosity,
        }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }
}

fn write_expected_assertions<W: io::Write>(writer: &mut W, result: &TestResult) {
    let failed_lines: HashSet<usize> =
        if let TestOutcome::Failed { assertions, .. } = &result.outcome {
            assertions.iter().map(|a| a.line).collect()
        } else {
            HashSet::new()
        };
    for a in &result.test.expected_assertions {
        let not_part = if a.negated { "not_." } else { "" };
        let args_str = a.args.join(", ");
        let assertion = format!(
            "expect({}).{}{}({})",
            a.subject, not_part, a.matcher, args_str
        );
        let text = a.label.as_deref().unwrap_or(&assertion);
        if failed_lines.contains(&(a.line as usize)) {
            let _ = writeln!(writer, "  {} {}", "✗".red(), text.dimmed());
        } else {
            let _ = writeln!(writer, "  {} {}", "✓".green(), text.dimmed());
        }
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
        let file = result.test.file_path.as_ref();
        if file != self.current_file.as_ref() {
            if self.current_file.is_some() && !matches!(self.verbosity, Verbosity::Quiet) {
                let _ = writeln!(self.writer);
            }
            if let Some(path) = file
                && !matches!(self.verbosity, Verbosity::Quiet)
            {
                let _ = writeln!(self.writer, "{}:", path.display());
            }
            self.current_file = file.cloned();
        }
        let display = result
            .test
            .display_name
            .as_deref()
            .unwrap_or(&result.test.name);
        match &result.outcome {
            TestOutcome::Passed => {
                if !matches!(self.verbosity, Verbosity::Quiet) {
                    let _ = writeln!(
                        self.writer,
                        "{} {} {}",
                        "✓".green(),
                        display.bold(),
                        format!("[{}]", format_duration(result.duration)).dimmed()
                    );
                    if matches!(self.verbosity, Verbosity::Verbose) {
                        write_expected_assertions(&mut self.writer, result);
                    }
                }
            }
            TestOutcome::Failed { assertions, .. } => {
                let _ = writeln!(
                    self.writer,
                    "{} {} {}",
                    "✗".red(),
                    display.bold(),
                    format!("[{}]", format_duration(result.duration)).dimmed()
                );
                if matches!(self.verbosity, Verbosity::Verbose) {
                    write_expected_assertions(&mut self.writer, result);
                }
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
                if !matches!(self.verbosity, Verbosity::Quiet) {
                    let _ = writeln!(
                        self.writer,
                        "{} {}",
                        "»".yellow().dimmed(),
                        display.dimmed()
                    );
                }
            }
        }
    }

    fn on_collect_complete(&mut self, tests: &[TestItem]) {
        let _ = writeln!(
            self.writer,
            "{} {}",
            "tryke test".bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        );
        let _ = writeln!(self.writer);
        let mut current_file: Option<&std::path::Path> = None;
        for test in tests {
            let file = test.file_path.as_deref();
            if file != current_file {
                if current_file.is_some() {
                    let _ = writeln!(self.writer);
                }
                if let Some(path) = file {
                    let _ = writeln!(self.writer, "{}:", path.display());
                }
                current_file = file;
            }
            let display = test.display_name.as_deref().unwrap_or(&test.name);
            let _ = writeln!(self.writer, "  {}", display.dimmed());
        }
        let _ = writeln!(self.writer);
        let _ = writeln!(self.writer, "{} tests collected.", tests.len());
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
                display_name: None,
                expected_assertions: vec![],
            },
            TestItem {
                name: "test_b".into(),
                module_path: "tests.m".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
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
                display_name: None,
                expected_assertions: vec![],
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
                display_name: None,
                expected_assertions: vec![],
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
                display_name: None,
                expected_assertions: vec![],
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
            display_name: None,
            expected_assertions: vec![],
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
                display_name: None,
                expected_assertions: vec![],
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
                display_name: None,
                expected_assertions: vec![],
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
    fn collect_only_lists_test_ids() {
        let mut r = reporter();
        let tests = vec![
            TestItem {
                name: "test_add".into(),
                module_path: "tests.math".into(),
                file_path: Some("tests/math.py".into()),
                line_number: Some(5),
                display_name: None,
                expected_assertions: vec![],
            },
            TestItem {
                name: "test_sub".into(),
                module_path: "tests.math".into(),
                file_path: Some("tests/math.py".into()),
                line_number: Some(10),
                display_name: None,
                expected_assertions: vec![],
            },
        ];
        r.on_collect_complete(&tests);
        let out = output(&r);
        assert!(out.contains("tests/math.py:"));
        assert!(out.contains("test_add"));
        assert!(out.contains("test_sub"));
        assert!(out.contains("2 tests collected."));
        let header_pos = out.find("tests/math.py:").unwrap();
        let add_pos = out.find("test_add").unwrap();
        let sub_pos = out.find("test_sub").unwrap();
        assert!(header_pos < add_pos);
        assert!(header_pos < sub_pos);
    }

    #[test]
    fn collect_groups_by_file() {
        let mut r = reporter();
        let make = |name: &str, file: &str| TestItem {
            name: name.into(),
            module_path: "tests.m".into(),
            file_path: Some(PathBuf::from(file)),
            line_number: None,
            display_name: None,
            expected_assertions: vec![],
        };
        r.on_collect_complete(&[
            make("test_a", "tests/a.py"),
            make("test_b", "tests/a.py"),
            make("test_c", "tests/b.py"),
        ]);
        let out = output(&r);
        let a_header = out.find("tests/a.py:").unwrap();
        let b_header = out.find("tests/b.py:").unwrap();
        assert!(a_header < out.find("test_a").unwrap());
        assert!(a_header < out.find("test_b").unwrap());
        assert!(b_header < out.find("test_c").unwrap());
        assert!(out.find("test_b").unwrap() < b_header);
        assert!(
            !out.contains("tests/a.py::test_a"),
            "should not show full id"
        );
    }

    #[test]
    fn groups_by_file() {
        let mut r = reporter();
        let make = |name: &str, file: &str| TestResult {
            test: TestItem {
                name: name.into(),
                module_path: "tests.m".into(),
                file_path: Some(PathBuf::from(file)),
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        };
        r.on_test_complete(&make("test_a", "tests/a.py"));
        r.on_test_complete(&make("test_b", "tests/a.py"));
        r.on_test_complete(&make("test_c", "tests/b.py"));

        let out = output(&r);
        let a_header = out.find("tests/a.py:").unwrap();
        let b_header = out.find("tests/b.py:").unwrap();
        let test_a = out.find("test_a").unwrap();
        let test_b = out.find("test_b").unwrap();
        let test_c = out.find("test_c").unwrap();
        assert!(a_header < test_a);
        assert!(a_header < test_b);
        assert!(b_header < test_c);
        assert!(
            test_b < b_header,
            "b.py header should appear after a.py tests"
        );
    }

    fn make_passed(name: &str, assertions: Vec<tryke_types::ExpectedAssertion>) -> TestResult {
        TestResult {
            test: TestItem {
                name: name.into(),
                module_path: "tests.m".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: assertions,
            },
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn make_assertion(
        subject: &str,
        matcher: &str,
        args: Vec<&str>,
    ) -> tryke_types::ExpectedAssertion {
        tryke_types::ExpectedAssertion {
            subject: subject.into(),
            matcher: matcher.into(),
            negated: false,
            args: args.into_iter().map(String::from).collect(),
            line: 1,
            label: None,
        }
    }

    #[test]
    fn verbose_shows_expected_assertions_on_pass() {
        let mut r = TextReporter::with_writer_and_verbosity(Vec::new(), Verbosity::Verbose);
        r.on_test_complete(&make_passed(
            "test_add",
            vec![make_assertion("add(1, 1)", "to_equal", vec!["2"])],
        ));
        let out = String::from_utf8_lossy(&r.into_writer()).into_owned();
        assert!(out.contains("✓"));
        assert!(out.contains("expect(add(1, 1)).to_equal(2)"));
    }

    #[test]
    fn normal_hides_expected_assertions() {
        let mut r = reporter();
        r.on_test_complete(&make_passed(
            "test_add",
            vec![make_assertion("add(1, 1)", "to_equal", vec!["2"])],
        ));
        let out = output(&r);
        assert!(!out.contains("expect("));
    }

    #[test]
    fn quiet_hides_pass_lines() {
        let mut r = TextReporter::with_writer_and_verbosity(Vec::new(), Verbosity::Quiet);
        r.on_test_complete(&make_passed("test_add", vec![]));
        let out = String::from_utf8_lossy(&r.into_writer()).into_owned();
        assert!(!out.contains("test_add"));
        assert!(!out.contains("✓"));
    }

    #[test]
    fn quiet_still_shows_failures() {
        let mut r = TextReporter::with_writer_and_verbosity(Vec::new(), Verbosity::Quiet);
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_fail".into(),
                module_path: "tests.m".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
            outcome: TestOutcome::Failed {
                message: "oops".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = String::from_utf8_lossy(&r.into_writer()).into_owned();
        assert!(out.contains("✗"));
        assert!(out.contains("test_fail"));
    }

    #[test]
    fn verbose_shows_negated_assertion() {
        let mut r = TextReporter::with_writer_and_verbosity(Vec::new(), Verbosity::Verbose);
        r.on_test_complete(&make_passed(
            "test_neg",
            vec![tryke_types::ExpectedAssertion {
                subject: "x".into(),
                matcher: "to_be_none".into(),
                negated: true,
                args: vec![],
                line: 1,
                label: None,
            }],
        ));
        let out = String::from_utf8_lossy(&r.into_writer()).into_owned();
        assert!(out.contains("✓"));
        assert!(out.contains("expect(x).not_.to_be_none()"));
    }

    #[test]
    fn display_name_shown_instead_of_name() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_fn".into(),
                module_path: "tests.m".into(),
                file_path: None,
                line_number: None,
                display_name: Some("my fancy test".into()),
                expected_assertions: vec![],
            },
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(&r);
        assert!(out.contains("my fancy test"));
        assert!(!out.contains("test_fn"));
    }

    #[test]
    fn verbose_shows_labeled_assertion() {
        let mut r = TextReporter::with_writer_and_verbosity(Vec::new(), Verbosity::Verbose);
        r.on_test_complete(&make_passed(
            "test_add",
            vec![tryke_types::ExpectedAssertion {
                subject: "x".into(),
                matcher: "to_equal".into(),
                negated: false,
                args: vec!["1".into()],
                line: 1,
                label: Some("sum check".into()),
            }],
        ));
        let out = String::from_utf8_lossy(&r.into_writer()).into_owned();
        assert!(out.contains("✓"));
        assert!(out.contains("sum check"));
        assert!(!out.contains("expect(x)"));
    }

    #[test]
    fn verbose_shows_failed_assertion_with_x() {
        let mut r = TextReporter::with_writer_and_verbosity(Vec::new(), Verbosity::Verbose);
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_fail".into(),
                module_path: "tests.m".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: vec![tryke_types::ExpectedAssertion {
                    subject: "x".into(),
                    matcher: "to_equal".into(),
                    negated: false,
                    args: vec!["1".into()],
                    line: 5,
                    label: None,
                }],
            },
            outcome: TestOutcome::Failed {
                message: "assertion failed".into(),
                assertions: vec![Assertion {
                    expression: "expect(x).to_equal(1)".into(),
                    file: None,
                    line: 5,
                    span_offset: 0,
                    span_length: 1,
                    expected: "1".into(),
                    received: "2".into(),
                }],
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = String::from_utf8_lossy(&r.into_writer()).into_owned();
        assert!(out.contains("✗"));
        assert!(out.contains("expect(x).to_equal(1)"));
    }

    #[test]
    fn verbose_shows_mixed_pass_fail() {
        let mut r = TextReporter::with_writer_and_verbosity(Vec::new(), Verbosity::Verbose);
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_mixed".into(),
                module_path: "tests.m".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: vec![
                    tryke_types::ExpectedAssertion {
                        subject: "a".into(),
                        matcher: "to_equal".into(),
                        negated: false,
                        args: vec!["1".into()],
                        line: 3,
                        label: None,
                    },
                    tryke_types::ExpectedAssertion {
                        subject: "b".into(),
                        matcher: "to_equal".into(),
                        negated: false,
                        args: vec!["2".into()],
                        line: 4,
                        label: None,
                    },
                ],
            },
            outcome: TestOutcome::Failed {
                message: "assertion failed".into(),
                assertions: vec![Assertion {
                    expression: "expect(b).to_equal(2)".into(),
                    file: None,
                    line: 4,
                    span_offset: 0,
                    span_length: 1,
                    expected: "2".into(),
                    received: "3".into(),
                }],
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = String::from_utf8_lossy(&r.into_writer()).into_owned();
        let line_a = out.lines().find(|l| l.contains("expect(a)")).unwrap();
        let line_b = out.lines().find(|l| l.contains("expect(b)")).unwrap();
        assert!(line_a.contains("✓"), "expect(a) line should have pass icon");
        assert!(line_b.contains("✗"), "expect(b) line should have fail icon");
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
