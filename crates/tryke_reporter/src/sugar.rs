//! pytest-sugar-style reporter: one progressively-built line per test
//! file with inline check/cross marks, deferred failures recap at the
//! end. Tests inside a file arrive in one batch (the execution layer
//! flushes per-file), so a sugar line is rendered fully-formed when a
//! file completes.

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;
use crate::diagnostic::{render_assertions, render_error_message, render_failure_message};
use crate::live::{LiveBar, format_elapsed, render_bar, supports_live};
use crate::summary;

const BAR_WIDTH: usize = 20;
const FILE_LINE_TARGET_WIDTH: usize = 80;

pub struct SugarReporter<W: Write = io::Stdout> {
    writer: W,
    bar: LiveBar,
    enabled: bool,
    total_tests: usize,
    completed_tests: usize,
    total_files: usize,
    completed_files: usize,
    current_file: Option<PathBuf>,
    current_marks: Vec<String>,
    failures: Vec<TestResult>,
    start: Instant,
    subcommand_label: &'static str,
    watch_hint: Option<String>,
}

impl SugarReporter {
    #[must_use]
    pub fn new() -> Self {
        let enabled = supports_live();
        Self {
            writer: io::stdout(),
            bar: LiveBar::new(enabled),
            enabled,
            total_tests: 0,
            completed_tests: 0,
            total_files: 0,
            completed_files: 0,
            current_file: None,
            current_marks: Vec::new(),
            failures: Vec::new(),
            start: Instant::now(),
            subcommand_label: "tryke test",
            watch_hint: None,
        }
    }
}

impl Default for SugarReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: Write> SugarReporter<W> {
    pub fn with_writer(writer: W) -> Self {
        Self {
            writer,
            bar: LiveBar::new(false),
            enabled: false,
            total_tests: 0,
            completed_tests: 0,
            total_files: 0,
            completed_files: 0,
            current_file: None,
            current_marks: Vec::new(),
            failures: Vec::new(),
            start: Instant::now(),
            subcommand_label: "tryke test",
            watch_hint: None,
        }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }

    fn draw_bar(&mut self) {
        if !self.enabled {
            return;
        }
        let bar = render_bar(self.completed_tests, self.total_tests, BAR_WIDTH);
        let elapsed = format_elapsed(self.start.elapsed());
        let line = format!(
            "{} [{}] [{}] {}/{} files — {}/{} tests",
            "Running".bold(),
            elapsed.dimmed(),
            bar,
            self.completed_files,
            self.total_files,
            self.completed_tests,
            self.total_tests,
        );
        self.bar.redraw(&line);
    }

    fn commit_current_file(&mut self) {
        let Some(file) = self.current_file.take() else {
            return;
        };
        let marks = std::mem::take(&mut self.current_marks);
        self.completed_files += 1;

        let path_str = file.display().to_string();
        let marks_plain_len: usize = marks
            .iter()
            .map(|m| strip_ansi_count_chars(m))
            .sum::<usize>();

        let pct = if self.total_tests == 0 {
            0
        } else {
            (self.completed_tests * 100) / self.total_tests
        };
        let bar = render_bar(self.completed_tests, self.total_tests, BAR_WIDTH / 2);
        let suffix = format!(" {pct:>3}% [{bar}]");

        let used = path_str.chars().count() + 1 + marks_plain_len + suffix.chars().count();
        let pad = FILE_LINE_TARGET_WIDTH.saturating_sub(used);
        let pad_str = " ".repeat(pad);

        let mut joined_marks = String::new();
        for m in &marks {
            joined_marks.push_str(m);
        }

        let _ = writeln!(
            self.writer,
            "{} {}{}{}",
            path_str.bold(),
            joined_marks,
            pad_str,
            suffix
        );
    }
}

fn outcome_mark(outcome: &TestOutcome) -> String {
    match outcome {
        TestOutcome::Passed => format!("{}", "✓".green()),
        TestOutcome::Failed { .. } => format!("{}", "✗".red().bold()),
        TestOutcome::Error { .. } => format!("{}", "E".red().bold()),
        TestOutcome::Skipped { .. } => format!("{}", "s".yellow()),
        TestOutcome::XFailed { .. } => format!("{}", "~".dimmed()),
        TestOutcome::XPassed => format!("{}", "X".red().bold()),
        TestOutcome::Todo { .. } => format!("{}", "T".cyan()),
    }
}

/// Count printable chars of `s`, skipping ANSI SGR sequences. Used so
/// padding math doesn't over-count escape codes.
fn strip_ansi_count_chars(s: &str) -> usize {
    let mut count = 0;
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            count += 1;
        }
    }
    count
}

impl<W: Write> Reporter for SugarReporter<W> {
    fn on_run_start(&mut self, tests: &[TestItem]) {
        self.total_tests = tests.len();
        self.completed_tests = 0;
        self.total_files = tests
            .iter()
            .filter_map(|t| t.file_path.as_ref())
            .collect::<HashSet<_>>()
            .len();
        self.completed_files = 0;
        self.current_file = None;
        self.current_marks.clear();
        self.failures.clear();
        self.start = Instant::now();

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
        // File transition: commit the previous file's line and start
        // accumulating for the new one. The execution layer guarantees
        // a file's tests arrive contiguously, so a change in
        // `file_path` reliably means "previous file done."
        let new_file = result.test.file_path.clone();
        if new_file != self.current_file {
            self.bar.clear();
            self.commit_current_file();
            self.current_file = new_file;
        }

        self.completed_tests += 1;
        self.current_marks.push(outcome_mark(&result.outcome));

        if matches!(
            result.outcome,
            TestOutcome::Failed { .. } | TestOutcome::Error { .. } | TestOutcome::XPassed
        ) {
            self.failures.push(result.clone());
        }

        self.draw_bar();
    }

    fn on_run_complete(&mut self, run_summary: &RunSummary) {
        // Commit the final file (which won't see a transition).
        self.bar.clear();
        self.commit_current_file();

        if !self.failures.is_empty() {
            let _ = writeln!(self.writer);
            let _ = writeln!(self.writer, "{}", "Failures:".red().bold());
            for fail in self.failures.clone() {
                write_failure(&mut self.writer, &fail);
            }
        }

        summary::write_summary_with_hint(&mut self.writer, run_summary, self.watch_hint.as_deref());
    }

    fn set_subcommand_label(&mut self, label: &'static str) {
        self.subcommand_label = label;
    }

    fn set_watch_hint(&mut self, hint: Option<String>) {
        self.watch_hint = hint;
    }
}

fn write_failure<W: Write>(writer: &mut W, fail: &TestResult) {
    let location = fail.test.file_path.as_deref().map_or_else(
        || fail.test.module_path.clone(),
        |p| p.display().to_string(),
    );
    let _ = writeln!(writer);
    let _ = writeln!(
        writer,
        "{} {} {}",
        "✗".red().bold(),
        fail.test.display_label(),
        format!("({location})").dimmed()
    );
    let test_file = fail
        .test
        .file_path
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned());
    match &fail.outcome {
        TestOutcome::Failed {
            message,
            traceback,
            assertions,
            ..
        } => {
            if !assertions.is_empty() {
                let mut buf = String::new();
                render_assertions(test_file.as_deref(), assertions, &mut buf);
                let _ = writer.write_all(buf.as_bytes());
            } else if !message.is_empty() {
                let mut buf = String::new();
                render_failure_message(message, traceback.as_deref(), false, &mut buf);
                let _ = writer.write_all(buf.as_bytes());
            }
        }
        TestOutcome::Error { message } => {
            let mut buf = String::new();
            render_error_message(message, &mut buf);
            let _ = writer.write_all(buf.as_bytes());
        }
        TestOutcome::XPassed => {
            let _ = writeln!(writer, "  XPASS (unexpected pass)");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;

    fn reporter() -> SugarReporter<Vec<u8>> {
        SugarReporter::with_writer(Vec::new())
    }

    fn output(r: SugarReporter<Vec<u8>>) -> String {
        String::from_utf8(r.into_writer()).expect("valid utf-8")
    }

    fn passed(name: &str, file: &str) -> TestResult {
        TestResult {
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
        }
    }

    fn failed(name: &str, file: &str) -> TestResult {
        TestResult {
            test: TestItem {
                name: name.into(),
                module_path: "tests.m".into(),
                file_path: Some(PathBuf::from(file)),
                ..Default::default()
            },
            outcome: TestOutcome::Failed {
                message: "boom".into(),
                traceback: None,
                assertions: vec![],
                executed_lines: vec![],
            },
            duration: Duration::from_millis(1),
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
    fn one_file_two_passes_emits_one_line_with_two_marks() {
        let mut r = reporter();
        r.on_run_start(&[
            passed("a", "tests/x.py").test.clone(),
            passed("b", "tests/x.py").test.clone(),
        ]);
        r.on_test_complete(&passed("a", "tests/x.py"));
        r.on_test_complete(&passed("b", "tests/x.py"));
        r.on_run_complete(&RunSummary {
            passed: 2,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(2),
            discovery_duration: None,
            test_duration: None,
            file_count: 1,
            start_time: None,
            changed_selection: None,
        });
        let out = output(r);
        assert!(out.contains("tests/x.py"), "out: {out}");
        let line = out
            .lines()
            .find(|l| l.contains("tests/x.py"))
            .expect("file line");
        // Two ✓ marks on the file line.
        assert_eq!(line.matches('✓').count(), 2, "line: {line}");
    }

    #[test]
    fn two_files_emit_two_lines() {
        let mut r = reporter();
        r.on_run_start(&[
            passed("a", "tests/x.py").test.clone(),
            passed("b", "tests/y.py").test.clone(),
        ]);
        r.on_test_complete(&passed("a", "tests/x.py"));
        r.on_test_complete(&passed("b", "tests/y.py"));
        r.on_run_complete(&RunSummary {
            passed: 2,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(2),
            discovery_duration: None,
            test_duration: None,
            file_count: 2,
            start_time: None,
            changed_selection: None,
        });
        let out = output(r);
        assert!(out.contains("tests/x.py"));
        assert!(out.contains("tests/y.py"));
    }

    #[test]
    fn failures_are_buffered_and_shown_at_end() {
        let mut r = reporter();
        let tests = vec![
            passed("a", "tests/x.py").test.clone(),
            failed("b", "tests/x.py").test.clone(),
        ];
        r.on_run_start(&tests);
        r.on_test_complete(&passed("a", "tests/x.py"));
        r.on_test_complete(&failed("b", "tests/x.py"));
        r.on_run_complete(&RunSummary {
            passed: 1,
            failed: 1,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(2),
            discovery_duration: None,
            test_duration: None,
            file_count: 1,
            start_time: None,
            changed_selection: None,
        });
        let out = output(r);
        let failures_idx = out.find("Failures:").expect("Failures section present");
        let summary_idx = out.find("FAIL").expect("summary badge present");
        // The "Failures:" recap should appear before the final summary badge.
        assert!(failures_idx < summary_idx);
        assert!(out.contains("boom"), "should include failure message");
        // The file line should have one ✓ and one ✗.
        let line = out
            .lines()
            .find(|l| l.starts_with("tests/x.py"))
            .or_else(|| out.lines().find(|l| l.contains("tests/x.py")))
            .expect("file line");
        assert!(line.contains('✓'));
        assert!(line.contains('✗'));
    }

    #[test]
    fn writer_has_no_cursor_escapes() {
        let mut r = reporter();
        let tests = vec![passed("a", "tests/x.py").test.clone()];
        r.on_run_start(&tests);
        r.on_test_complete(&passed("a", "tests/x.py"));
        r.on_run_complete(&RunSummary {
            passed: 1,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(1),
            discovery_duration: None,
            test_duration: None,
            file_count: 1,
            start_time: None,
            changed_selection: None,
        });
        let out = output(r);
        assert!(!out.contains("\x1b[2K"));
        assert!(!out.contains("\x1b[?25"));
    }

    #[test]
    fn empty_run_writes_nothing_for_files() {
        let mut r = reporter();
        r.on_run_start(&[]);
        r.on_run_complete(&RunSummary {
            passed: 0,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(1),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        let out = output(r);
        // No file lines, but the summary should still print.
        assert!(out.contains("PASS"));
    }

    #[test]
    fn outcome_mark_per_outcome() {
        assert!(outcome_mark(&TestOutcome::Passed).contains('✓'));
        assert!(
            outcome_mark(&TestOutcome::Failed {
                message: String::new(),
                traceback: None,
                assertions: vec![],
                executed_lines: vec![],
            })
            .contains('✗')
        );
        assert!(outcome_mark(&TestOutcome::Skipped { reason: None }).contains('s'));
        assert!(outcome_mark(&TestOutcome::Todo { description: None }).contains('T'));
    }

    #[test]
    fn strip_ansi_count_chars_skips_sgr() {
        assert_eq!(strip_ansi_count_chars("\x1b[32m✓\x1b[0m"), 1);
        assert_eq!(strip_ansi_count_chars("plain"), 5);
    }
}
