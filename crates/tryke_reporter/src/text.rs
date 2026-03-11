use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

use tryke_types::DiscoveryError;

use crate::Reporter;
use crate::diagnostic::{
    render_assertions, render_captured_output, render_error_message, render_failure_message,
};

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
    current_groups: Vec<String>,
    verbosity: Verbosity,
}

impl TextReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
            current_file: None,
            current_groups: Vec::new(),
            verbosity: Verbosity::Normal,
        }
    }

    #[must_use]
    pub fn with_verbosity(verbosity: Verbosity) -> Self {
        Self {
            writer: io::stdout(),
            current_file: None,
            current_groups: Vec::new(),
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
            current_groups: Vec::new(),
            verbosity: Verbosity::Normal,
        }
    }

    pub fn with_writer_and_verbosity(writer: W, verbosity: Verbosity) -> Self {
        Self {
            writer,
            current_file: None,
            current_groups: Vec::new(),
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

fn write_captured<W: io::Write>(writer: &mut W, label: &str, content: &str) {
    let mut buf = String::new();
    render_captured_output(label, content, &mut buf);
    let _ = write!(writer, "{buf}");
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
        self.current_file = None;
        self.current_groups.clear();
        let _ = writeln!(
            self.writer,
            "{} {}",
            "tryke test".bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        );
        let _ = writeln!(self.writer);
    }

    #[expect(clippy::too_many_lines)]
    fn on_test_complete(&mut self, result: &TestResult) {
        let file = result.test.file_path.as_ref();
        if file != self.current_file.as_ref() {
            if !matches!(self.verbosity, Verbosity::Quiet) {
                if self.current_file.is_some() {
                    let _ = writeln!(self.writer);
                }
                if let Some(path) = file {
                    let _ = writeln!(self.writer, "{}:", path.display());
                }
            }
            self.current_file = file.cloned();
            self.current_groups.clear();
        }

        // Print group headers when groups change
        let test_groups = &result.test.groups;
        if !matches!(self.verbosity, Verbosity::Quiet) && test_groups != &self.current_groups {
            // Find where the current and new group paths diverge
            let common = self
                .current_groups
                .iter()
                .zip(test_groups.iter())
                .take_while(|(a, b)| a == b)
                .count();
            // Print each new group header with indentation
            for (depth, group) in test_groups.iter().enumerate().skip(common) {
                let indent = "  ".repeat(depth + 1);
                let _ = writeln!(self.writer, "{indent}{group}");
            }
            self.current_groups.clone_from(test_groups);
        }

        let group_indent = if test_groups.is_empty() {
            String::new()
        } else {
            "  ".repeat(test_groups.len() + 1)
        };

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
                        "{group_indent}{} {} {}",
                        "✓".green(),
                        display,
                        format!("[{}]", format_duration(result.duration)).dimmed()
                    );
                    if matches!(self.verbosity, Verbosity::Verbose) {
                        write_expected_assertions(&mut self.writer, result);
                    }
                }
            }
            TestOutcome::Failed {
                message,
                traceback,
                assertions,
            } => {
                let _ = writeln!(
                    self.writer,
                    "{group_indent}{} {} {}",
                    "✗".red(),
                    display,
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
                } else if !message.is_empty() {
                    let verbose = matches!(self.verbosity, Verbosity::Verbose);
                    let mut buf = String::new();
                    render_failure_message(message, traceback.as_deref(), verbose, &mut buf);
                    let _ = write!(self.writer, "{buf}");
                }
                if !result.stdout.is_empty() {
                    write_captured(&mut self.writer, "stdout", &result.stdout);
                }
                if !result.stderr.is_empty() {
                    write_captured(&mut self.writer, "stderr", &result.stderr);
                }
            }
            TestOutcome::Error { message } => {
                let _ = writeln!(
                    self.writer,
                    "{group_indent}{} {} {}",
                    "!".red(),
                    display,
                    "[error]".red()
                );
                let mut buf = String::new();
                render_error_message(message, &mut buf);
                let _ = write!(self.writer, "{buf}");
                if !result.stderr.is_empty() {
                    write_captured(&mut self.writer, "stderr", &result.stderr);
                }
            }
            TestOutcome::Skipped { .. } => {
                if !matches!(self.verbosity, Verbosity::Quiet) {
                    let _ = writeln!(
                        self.writer,
                        "{group_indent}{} {}",
                        "»".yellow().dimmed(),
                        display.dimmed()
                    );
                }
            }
            TestOutcome::XFailed { .. } => {
                if !matches!(self.verbosity, Verbosity::Quiet) {
                    let _ = writeln!(
                        self.writer,
                        "{group_indent}{} {}",
                        "~".dimmed(),
                        display.dimmed()
                    );
                }
            }
            TestOutcome::XPassed => {
                let _ = writeln!(
                    self.writer,
                    "{group_indent}{} {} {}",
                    "!".red(),
                    display,
                    "XPASS (unexpected pass)".red()
                );
            }
            TestOutcome::Todo { .. } => {
                if !matches!(self.verbosity, Verbosity::Quiet) {
                    let _ = writeln!(
                        self.writer,
                        "{group_indent}{} {}",
                        "T".cyan(),
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
        let mut current_groups: Vec<String> = Vec::new();
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
                current_groups.clear();
            }
            if test.groups != current_groups {
                let common = current_groups
                    .iter()
                    .zip(test.groups.iter())
                    .take_while(|(a, b)| a == b)
                    .count();
                for (depth, group) in test.groups.iter().enumerate().skip(common) {
                    let indent = "  ".repeat(depth + 1);
                    let _ = writeln!(self.writer, "{indent}{group}");
                }
                current_groups.clone_from(&test.groups);
            }
            let group_indent = "  ".repeat(test.groups.len());
            let display = test.display_name.as_deref().unwrap_or(&test.name);
            let _ = writeln!(self.writer, "  {group_indent}{}", display.dimmed());
        }
        let _ = writeln!(self.writer);
        let _ = writeln!(self.writer, "{} tests collected.", tests.len());
    }

    fn on_run_complete(&mut self, summary: &RunSummary) {
        crate::summary::write_summary(&mut self.writer, summary);
    }

    fn on_discovery_error(&mut self, error: &DiscoveryError) {
        let _ = writeln!(
            self.writer,
            "{} {}: {}",
            "!".red(),
            error.file_path.display().to_string().yellow(),
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
                ..Default::default()
            },
            TestItem {
                name: "test_b".into(),
                module_path: "tests.m".into(),
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
            },
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
                ..Default::default()
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
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(100),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });

        let out = output(&r);
        assert!(out.contains("FAIL"));
        assert!(out.contains("Tests"));
        assert!(out.contains("1 failed"));
        assert!(out.contains("3 passed"));
        assert!(out.contains("2 skipped"));
        assert!(out.contains("(6)"));
        assert!(out.contains("Duration"));
    }

    #[test]
    fn run_complete_hides_zero_fail_and_skip() {
        let mut r = reporter();
        r.on_run_complete(&RunSummary {
            passed: 5,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(50),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });

        let out = output(&r);
        assert!(out.contains("PASS"));
        assert!(out.contains("5 passed"));
        assert!(!out.contains("failed"));
        assert!(!out.contains("skipped"));
        assert!(out.contains("(5)"));
    }

    #[test]
    fn full_lifecycle() {
        let mut r = reporter();
        let tests = vec![TestItem {
            name: "test_one".into(),
            module_path: "tests.mod_a".into(),
            ..Default::default()
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
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(10),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });

        let out = output(&r);
        assert!(out.contains("tryke test"));
        assert!(out.contains("✓"));
        assert!(out.contains("test_one"));
        assert!(out.contains("PASS"));
        assert!(out.contains("1 passed"));
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
                ..Default::default()
            },
            outcome: TestOutcome::Failed {
                message: "assertion failed".into(),
                traceback: None,
                assertions: vec![Assertion {
                    expression: "assert_eq!(a, 2)".into(),
                    file: None,
                    line: 10,
                    span_offset: 14,
                    span_length: 1,
                    expected: "2".into(),
                    received: "3".into(),
                    expected_arg_span: None,
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
                ..Default::default()
            },
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
                ..Default::default()
            },
            TestItem {
                name: "test_sub".into(),
                module_path: "tests.math".into(),
                file_path: Some("tests/math.py".into()),
                line_number: Some(10),
                ..Default::default()
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
            ..Default::default()
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
                ..Default::default()
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
                expected_assertions: assertions,
                ..Default::default()
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
                ..Default::default()
            },
            outcome: TestOutcome::Failed {
                message: "oops".into(),
                traceback: None,
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
                display_name: Some("my fancy test".into()),
                ..Default::default()
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
                expected_assertions: vec![tryke_types::ExpectedAssertion {
                    subject: "x".into(),
                    matcher: "to_equal".into(),
                    negated: false,
                    args: vec!["1".into()],
                    line: 5,
                    label: None,
                }],
                ..Default::default()
            },
            outcome: TestOutcome::Failed {
                message: "assertion failed".into(),
                traceback: None,
                assertions: vec![Assertion {
                    expression: "expect(x).to_equal(1)".into(),
                    file: None,
                    line: 5,
                    span_offset: 0,
                    span_length: 1,
                    expected: "1".into(),
                    received: "2".into(),
                    expected_arg_span: Some((19, 1)),
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
                ..Default::default()
            },
            outcome: TestOutcome::Failed {
                message: "assertion failed".into(),
                traceback: None,
                assertions: vec![Assertion {
                    expression: "expect(b).to_equal(2)".into(),
                    file: None,
                    line: 4,
                    span_offset: 0,
                    span_length: 1,
                    expected: "2".into(),
                    received: "3".into(),
                    expected_arg_span: Some((19, 1)),
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
