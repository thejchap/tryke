//! cargo-nextest-style reporter: one line per completed test, with a
//! live status bar redrawn at the bottom of the screen.
//!
//! Per-test lines are written to `self.writer` (default: stdout). The
//! status bar is written to stderr through a `LiveBar`, so snapshot
//! tests over the writer are free of cursor-control escapes.

use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;
use crate::diagnostic::{render_assertions, render_error_message, render_failure_message};
use crate::live::{LiveBar, format_elapsed, render_bar, supports_live};
use crate::summary;

const BAR_WIDTH: usize = 20;
const BADGE_WIDTH: usize = 5;

pub struct NextReporter<W: Write = io::Stdout> {
    writer: W,
    bar: LiveBar,
    enabled: bool,
    total: usize,
    completed: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
    left_col_width: usize,
    start: Instant,
    subcommand_label: &'static str,
}

impl NextReporter {
    #[must_use]
    pub fn new() -> Self {
        let enabled = supports_live();
        Self {
            writer: io::stdout(),
            bar: LiveBar::new(enabled),
            enabled,
            total: 0,
            completed: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            left_col_width: 0,
            start: Instant::now(),
            subcommand_label: "tryke test",
        }
    }
}

impl Default for NextReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: Write> NextReporter<W> {
    pub fn with_writer(writer: W) -> Self {
        Self {
            writer,
            // Tests inject a non-stdout writer; never draw to a real
            // stderr in that case (it'd corrupt test runner output).
            bar: LiveBar::new(false),
            enabled: false,
            total: 0,
            completed: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            left_col_width: 0,
            start: Instant::now(),
            subcommand_label: "tryke test",
        }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }

    fn draw_bar(&mut self) {
        if !self.enabled {
            return;
        }
        let bar = render_bar(self.completed, self.total, BAR_WIDTH);
        let elapsed = format_elapsed(self.start.elapsed());
        let line = format!(
            "{} [{}] [{}] {}/{} — {}, {}",
            "Running".bold(),
            elapsed.dimmed(),
            bar,
            self.completed,
            self.total,
            format!("{} passed", self.passed).green(),
            format!("{} failed", self.failed).red(),
        );
        self.bar.redraw(&line);
    }
}

/// Right-aligned `   0.009s` form (matches cargo-nextest).
fn format_test_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    format!("{secs:>7.3}s")
}

fn left_label(test: &TestItem) -> String {
    let stem = test
        .file_path
        .as_deref()
        .and_then(Path::file_stem)
        .map_or_else(
            || test.module_path.clone(),
            |s| s.to_string_lossy().into_owned(),
        );
    if test.groups.is_empty() {
        stem
    } else {
        format!("{} > {}", stem, test.groups.join(" > "))
    }
}

impl<W: Write> Reporter for NextReporter<W> {
    fn on_run_start(&mut self, tests: &[TestItem]) {
        self.total = tests.len();
        self.completed = 0;
        self.passed = 0;
        self.failed = 0;
        self.skipped = 0;
        self.start = Instant::now();
        // Pre-compute column-1 width once so per-test lines align
        // throughout the run with no jitter.
        self.left_col_width = tests.iter().map(|t| left_label(t).len()).max().unwrap_or(0);

        let _ = writeln!(
            self.writer,
            "{} {}",
            self.subcommand_label.bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        );
        let _ = writeln!(self.writer);

        self.draw_bar();
    }

    fn on_test_complete(&mut self, result: &TestResult) {
        // Tear the bar down before writing the per-test line, so the
        // line lands on a clean row instead of overwriting whatever was
        // last drawn on the bar's row.
        self.bar.clear();

        self.completed += 1;
        match &result.outcome {
            TestOutcome::Passed => self.passed += 1,
            TestOutcome::Failed { .. } | TestOutcome::Error { .. } | TestOutcome::XPassed => {
                self.failed += 1;
            }
            TestOutcome::Skipped { .. }
            | TestOutcome::XFailed { .. }
            | TestOutcome::Todo { .. } => self.skipped += 1,
        }

        let (badge, raw_badge): (String, &str) = match &result.outcome {
            TestOutcome::Passed => (format!("{}", "PASS ".green().bold()), "PASS "),
            TestOutcome::Failed { .. } => (format!("{}", "FAIL ".red().bold()), "FAIL "),
            TestOutcome::Error { .. } => (format!("{}", "ERROR".red().bold()), "ERROR"),
            TestOutcome::Skipped { .. } => (format!("{}", "SKIP ".yellow()), "SKIP "),
            TestOutcome::XFailed { .. } => (format!("{}", "XFAIL".dimmed()), "XFAIL"),
            TestOutcome::XPassed => (format!("{}", "XPASS".red().bold()), "XPASS"),
            TestOutcome::Todo { .. } => (format!("{}", "TODO ".cyan()), "TODO "),
        };
        debug_assert_eq!(raw_badge.len(), BADGE_WIDTH);

        let dur = format_test_duration(result.duration);
        let left = left_label(&result.test);
        let pad = self.left_col_width.saturating_sub(left.len());
        let display = result.test.display_label();

        let suffix_text = match &result.outcome {
            TestOutcome::Skipped {
                reason: Some(reason),
            }
            | TestOutcome::XFailed {
                reason: Some(reason),
            } => Some(reason.as_str()),
            TestOutcome::Todo {
                description: Some(desc),
            } => Some(desc.as_str()),
            _ => None,
        };
        let suffix =
            suffix_text.map_or_else(String::new, |t| format!(" {}", format!("({t})").dimmed()));

        let _ = writeln!(
            self.writer,
            "{badge} [{}] {}{} :: {}{}",
            dur.dimmed(),
            left,
            " ".repeat(pad),
            display,
            suffix
        );

        // Inline failure detail right after the line — keeps cause near
        // effect, like nextest does.
        match &result.outcome {
            TestOutcome::Failed {
                message,
                traceback,
                assertions,
                ..
            } => {
                let test_file = result
                    .test
                    .file_path
                    .as_deref()
                    .map(|p| p.to_string_lossy().into_owned());
                if !assertions.is_empty() {
                    let mut buf = String::new();
                    render_assertions(test_file.as_deref(), assertions, &mut buf);
                    let _ = self.writer.write_all(buf.as_bytes());
                } else if !message.is_empty() {
                    let mut buf = String::new();
                    render_failure_message(message, traceback.as_deref(), false, &mut buf);
                    let _ = self.writer.write_all(buf.as_bytes());
                }
            }
            TestOutcome::Error { message } => {
                let mut buf = String::new();
                render_error_message(message, &mut buf);
                let _ = self.writer.write_all(buf.as_bytes());
            }
            _ => {}
        }

        self.draw_bar();
    }

    fn on_run_complete(&mut self, run_summary: &RunSummary) {
        self.bar.clear();
        summary::write_summary(&mut self.writer, run_summary);
    }

    fn set_subcommand_label(&mut self, label: &'static str) {
        self.subcommand_label = label;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tryke_types::Assertion;

    use super::*;

    fn reporter() -> NextReporter<Vec<u8>> {
        NextReporter::with_writer(Vec::new())
    }

    fn output(r: NextReporter<Vec<u8>>) -> String {
        String::from_utf8(r.into_writer()).expect("valid utf-8")
    }

    fn passed(name: &str) -> TestResult {
        TestResult {
            test: TestItem {
                name: name.into(),
                module_path: "tests.m".into(),
                file_path: Some(PathBuf::from(format!("tests/{name}.py"))),
                ..Default::default()
            },
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(9),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    #[test]
    fn run_start_emits_header() {
        let mut r = reporter();
        r.on_run_start(&[]);
        let out = output(r);
        assert!(out.contains("tryke test"));
    }

    #[test]
    fn pass_line_has_pass_badge_and_duration() {
        let mut r = reporter();
        r.on_test_complete(&passed("test_one"));
        let out = output(r);
        assert!(out.contains("PASS"));
        assert!(out.contains("test_one"));
        assert!(out.contains("0.009s"));
    }

    #[test]
    fn fail_line_has_fail_badge_and_diagnostics() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_bad".into(),
                module_path: "tests.m".into(),
                file_path: Some(PathBuf::from("tests/m.py")),
                ..Default::default()
            },
            outcome: TestOutcome::Failed {
                message: "expected 1, got 2".into(),
                traceback: None,
                assertions: vec![],
                executed_lines: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(r);
        assert!(out.contains("FAIL"));
        assert!(out.contains("test_bad"));
        assert!(out.contains("expected 1, got 2"));
    }

    #[test]
    fn skip_line_has_skip_badge_and_reason() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "test_skip".into(),
                module_path: "tests.m".into(),
                ..Default::default()
            },
            outcome: TestOutcome::Skipped {
                reason: Some("not on linux".into()),
            },
            duration: Duration::ZERO,
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(r);
        assert!(out.contains("SKIP"));
        assert!(out.contains("not on linux"));
    }

    #[test]
    fn left_column_pads_to_widest_label() {
        let mut r = reporter();
        let tests = vec![
            TestItem {
                name: "t1".into(),
                module_path: "tests.m".into(),
                file_path: Some(PathBuf::from("tests/short.py")),
                ..Default::default()
            },
            TestItem {
                name: "t2".into(),
                module_path: "tests.m".into(),
                file_path: Some(PathBuf::from("tests/very_long_filename.py")),
                ..Default::default()
            },
        ];
        r.on_run_start(&tests);
        // Width should be the longer stem
        assert_eq!(r.left_col_width, "very_long_filename".len());
    }

    #[test]
    fn writer_has_no_cursor_escapes() {
        // Bar goes to stderr, so the writer should never see clear-line
        // or hide-cursor codes — important for snapshot stability.
        let mut r = reporter();
        r.on_run_start(&[passed("a").test.clone()]);
        r.on_test_complete(&passed("a"));
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
        let out = output(r);
        assert!(!out.contains("\x1b[2K"));
        assert!(!out.contains("\x1b[?25"));
        assert!(!out.contains("\r\x1b"));
    }

    #[test]
    fn run_complete_writes_summary() {
        let mut r = reporter();
        r.on_run_complete(&RunSummary {
            passed: 3,
            failed: 1,
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
        let out = output(r);
        assert!(out.contains("FAIL"));
        assert!(out.contains("1 failed"));
        assert!(out.contains("3 passed"));
    }

    #[test]
    fn left_label_uses_groups() {
        let test = TestItem {
            name: "t".into(),
            module_path: "tests.m".into(),
            file_path: Some(PathBuf::from("tests/test_math.py")),
            groups: vec!["Math".into(), "addition".into()],
            ..Default::default()
        };
        assert_eq!(left_label(&test), "test_math > Math > addition");
    }

    #[test]
    fn left_label_falls_back_to_module() {
        let test = TestItem {
            name: "t".into(),
            module_path: "tests.fallback".into(),
            file_path: None,
            ..Default::default()
        };
        assert_eq!(left_label(&test), "tests.fallback");
    }

    #[test]
    fn case_label_appears_in_test_id() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "square".into(),
                module_path: "tests.m".into(),
                case_label: Some("zero".into()),
                case_index: Some(0),
                ..Default::default()
            },
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(r);
        assert!(out.contains("square[zero]"), "out: {out}");
    }

    #[test]
    fn failed_with_assertion_renders_diagnostic() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: TestItem {
                name: "t".into(),
                module_path: "tests.m".into(),
                file_path: Some(PathBuf::from("tests/m.py")),
                ..Default::default()
            },
            outcome: TestOutcome::Failed {
                message: "x".into(),
                traceback: None,
                assertions: vec![Assertion {
                    expression: "expect(1).to_equal(2)".into(),
                    file: None,
                    line: 1,
                    span_offset: 7,
                    span_length: 1,
                    expected: "2".into(),
                    received: "1".into(),
                    expected_arg_span: None,
                }],
                executed_lines: vec![],
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        let out = output(r);
        assert!(out.contains("expected 2, received 1"));
    }

    #[test]
    fn format_test_duration_pads_under_a_second() {
        assert_eq!(format_test_duration(Duration::from_millis(9)), "  0.009s");
    }

    #[test]
    fn format_test_duration_seconds() {
        assert_eq!(
            format_test_duration(Duration::from_millis(12_345)),
            " 12.345s"
        );
    }
}
