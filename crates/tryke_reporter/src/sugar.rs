//! pytest-sugar-style reporter: one progressively-built line per test
//! file with inline check/cross marks, deferred failures recap at the
//! end. Tests inside a file arrive in one batch (the execution layer
//! flushes per-file), so a sugar line is rendered fully-formed when a
//! file completes.
//!
//! Per-file rows and the bottom progress bar both flow through a shared
//! [`LiveArea`], so each `println` clears the bar, prints above it, and
//! redraws atomically.

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;
use crate::diagnostic::{render_assertions, render_error_message, render_failure_message};
use crate::live::{LiveArea, render_bar};
use crate::summary;

const SUFFIX_BAR_WIDTH: usize = 12;

/// Bar template — pytest-sugar-style pipe-bracketed bar with a white
/// fill (`{wide_bar}` so it stretches to fill the terminal width). We
/// swap to `red_bar_template` once a failure is observed.
fn white_bar_template() -> String {
    format!(
        "   {} |{{wide_bar:.white/dim}}| {{percent:>3}}% \x1b[2m·\x1b[0m \
         {{pos}}/{{len}} tests \x1b[2m·\x1b[0m {{prefix}} files \
         \x1b[2m·\x1b[0m {{elapsed_precise:.dim}}",
        "Progress:".bold(),
    )
}

fn red_bar_template() -> String {
    format!(
        "   {} |{{wide_bar:.red/dim}}| {{percent:>3}}% \x1b[2m·\x1b[0m \
         {{pos}}/{{len}} tests \x1b[2m·\x1b[0m {{prefix}} files \
         \x1b[2m·\x1b[0m {{elapsed_precise:.dim}}",
        "Progress:".bold(),
    )
}

pub struct SugarReporter<W: Write = io::Stdout> {
    writer: W,
    live: LiveArea,
    started: bool,
    failure_seen: bool,
    total_tests: u64,
    completed_tests: u64,
    total_files: u64,
    completed_files: u64,
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
        Self {
            writer: io::stdout(),
            live: LiveArea::new(),
            started: false,
            failure_seen: false,
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
            live: LiveArea::hidden(),
            started: false,
            failure_seen: false,
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

    fn ensure_bar_started(&mut self) {
        if self.started {
            return;
        }
        self.live.start(self.total_tests, &white_bar_template());
        self.live
            .set_prefix(format!("{}/{}", self.completed_files, self.total_files));
        self.started = true;
    }

    fn refresh_bar(&self) {
        self.live.set_position(self.completed_tests);
        self.live
            .set_prefix(format!("{}/{}", self.completed_files, self.total_files));
    }

    fn note_failure(&mut self) {
        if !self.failure_seen {
            self.failure_seen = true;
            self.live.set_template(&red_bar_template());
        }
    }

    fn commit_current_file(&mut self) {
        let Some(file) = self.current_file.take() else {
            return;
        };
        let marks = std::mem::take(&mut self.current_marks);
        self.completed_files += 1;

        let path_str = file.display().to_string();
        let term_width = self.live.width().max(40);

        let pct = if self.total_tests == 0 {
            0
        } else {
            (self.completed_tests * 100) / self.total_tests
        };
        let bar = render_bar(
            usize::try_from(self.completed_tests).unwrap_or(usize::MAX),
            usize::try_from(self.total_tests).unwrap_or(usize::MAX),
            SUFFIX_BAR_WIDTH,
        );

        let count_str = marks.len().to_string();
        let pct_str = format!("{pct:>3}%");
        // Plain (ANSI-stripped) rendering of the suffix for width math.
        let suffix_plain_len =
            2 + count_str.chars().count() + 1 + pct_str.chars().count() + 1 + bar.chars().count();
        let suffix_styled = format!(
            "  {} {} {}",
            count_str.bold(),
            pct_str.bold(),
            if self.failure_seen {
                format!("{}", bar.red())
            } else {
                format!("{}", bar.white())
            }
        );

        // Marks rendered with no separator — almost-touching, matches
        // pytest-sugar's compact look.
        let marks_joined: String = marks.iter().map(String::as_str).collect();
        let marks_plain_len: usize = marks
            .iter()
            .map(|m| strip_ansi_count_chars(m))
            .sum::<usize>();

        let path_plain_len = path_str.chars().count();
        // " <path> <marks>" prefix length, plain (no ANSI).
        let prefix_plain_len = 1 + path_plain_len + 1 + marks_plain_len;

        let line = if prefix_plain_len + suffix_plain_len <= term_width {
            // Single-line layout: pad between marks and suffix so the
            // suffix sits flush at the right edge of the terminal.
            let pad = term_width - prefix_plain_len - suffix_plain_len;
            format!(
                " {} {marks_joined}{}{suffix_styled}",
                path_str.bold(),
                " ".repeat(pad)
            )
        } else {
            // Marks overflowed the terminal width: drop the suffix to a
            // continuation line, right-aligned.
            let pad = term_width.saturating_sub(suffix_plain_len);
            format!(
                " {} {marks_joined}\n{}{suffix_styled}",
                path_str.bold(),
                " ".repeat(pad)
            )
        };

        for row in line.split('\n') {
            self.live.println(&mut self.writer, row);
        }
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
        self.total_tests = tests.len() as u64;
        self.completed_tests = 0;
        self.total_files = tests
            .iter()
            .filter_map(|t| t.file_path.as_ref())
            .collect::<HashSet<_>>()
            .len() as u64;
        self.completed_files = 0;
        self.current_file = None;
        self.current_marks.clear();
        self.failures.clear();
        self.start = Instant::now();
        self.started = false;
        self.failure_seen = false;

        let header = format!(
            "{} {}",
            self.subcommand_label.bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        );
        self.live.println(&mut self.writer, &header);
        self.live.println(&mut self.writer, "");
    }

    fn on_test_complete(&mut self, result: &TestResult) {
        self.ensure_bar_started();

        // File transition: commit the previous file's accumulated marks
        // as a single row, then start fresh. The execution layer
        // guarantees a file's tests arrive contiguously, so a change in
        // `file_path` reliably means "previous file done."
        let new_file = result.test.file_path.clone();
        if new_file != self.current_file {
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
            self.note_failure();
        }

        self.refresh_bar();
    }

    fn on_run_complete(&mut self, run_summary: &RunSummary) {
        self.commit_current_file();
        self.live.finish_and_clear();

        if !self.failures.is_empty() {
            self.live.println(&mut self.writer, "");
            // Pytest-sugar-style failures header — red bold underline.
            let header = format!("{}", "Failures".red().bold().underline());
            self.live.println(&mut self.writer, &header);
            for fail in &self.failures {
                write_failure(&self.live, &mut self.writer, fail);
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

fn write_failure<W: Write>(live: &LiveArea, writer: &mut W, fail: &TestResult) {
    let location = fail.test.file_path.as_deref().map_or_else(
        || fail.test.module_path.clone(),
        |p| p.display().to_string(),
    );
    live.println(writer, "");
    let header = format!(
        "{} {} {}",
        "✗".red().bold(),
        fail.test.display_label(),
        format!("({location})").dimmed()
    );
    live.println(writer, &header);

    let test_file = fail
        .test
        .file_path
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned());
    let detail = match &fail.outcome {
        TestOutcome::Failed {
            message,
            traceback,
            assertions,
            ..
        } => {
            let mut buf = String::new();
            if !assertions.is_empty() {
                render_assertions(test_file.as_deref(), assertions, &mut buf);
            } else if !message.is_empty() {
                render_failure_message(message, traceback.as_deref(), false, &mut buf);
            }
            buf
        }
        TestOutcome::Error { message } => {
            let mut buf = String::new();
            render_error_message(message, &mut buf);
            buf
        }
        TestOutcome::XPassed => "  XPASS (unexpected pass)\n".to_string(),
        _ => String::new(),
    };
    for line in detail.lines() {
        live.println(writer, line);
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
        let failures_idx = out.find("Failures").expect("Failures section present");
        let summary_idx = out.find("FAIL").expect("summary badge present");
        assert!(failures_idx < summary_idx);
        assert!(out.contains("boom"), "should include failure message");
        let line = out
            .lines()
            .find(|l| l.contains("tests/x.py"))
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
