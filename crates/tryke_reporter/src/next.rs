//! cargo-nextest-style reporter: one line per completed test, with a
//! live status bar redrawn at the bottom of the screen.
//!
//! Per-test rows and the bar both flow through a [`LiveArea`] (backed
//! by [`indicatif::MultiProgress`]) so they coordinate atomically: each
//! `println` clears the bar, prints above it, and redraws — fixing the
//! stdout/stderr cursor-desync that left the bar invisible in PR #70's
//! original hand-rolled implementation.

use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;
use crate::diagnostic::{render_assertions, render_error_message, render_failure_message};
use crate::live::LiveArea;
use crate::summary;

const BADGE_WIDTH: usize = 5;
const SLOW_TEST_THRESHOLD: Duration = Duration::from_secs(1);
const VERY_SLOW_TEST_THRESHOLD: Duration = Duration::from_secs(5);

/// Build the bar template at runtime so the bar fills whatever
/// horizontal space the terminal offers — wide terminals get a wide
/// white bar, narrow ones still leave at least 5 cells before
/// indicatif would truncate. White matches the user's request and
/// stays neutral against any 256-color terminal scheme.
fn build_bar_template(term_width: usize) -> String {
    // Reserved plain-text overhead: 5-space indent + "Running" (7) +
    // " [" (2) + "HH:MM:SS" (8) + "] [" (3) + "] " (2) + "{pos:>4}/
    // {len:<4}" (9) + 1 trailing space + ~25 chars allowance for the
    // colored `{msg}` tail. Whatever's left over goes to the bar.
    let reserved = 5 + 7 + 2 + 8 + 3 + 2 + 9 + 1 + 25;
    let bar_width = term_width.saturating_sub(reserved).max(5);
    format!(
        "     {{prefix:.cyan.bold}} [{{elapsed_precise:.dim}}] \
         [{{bar:{bar_width}.white/dim}}] {{pos:>4}}/{{len:<4}} {{msg}}"
    )
}

pub struct NextReporter<W: Write = io::Stdout> {
    writer: W,
    live: LiveArea,
    started: bool,
    total: u64,
    completed: u64,
    passed: u64,
    failed: u64,
    skipped: u64,
    start: Instant,
    subcommand_label: &'static str,
    watch_hint: Option<String>,
}

impl NextReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
            live: LiveArea::new(),
            started: false,
            total: 0,
            completed: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            start: Instant::now(),
            subcommand_label: "tryke test",
            watch_hint: None,
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
            // Snapshot/test mode: no live bar, plain `writeln!` to the
            // caller's writer.
            live: LiveArea::hidden(),
            started: false,
            total: 0,
            completed: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
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
        let template = build_bar_template(self.live.width());
        self.live.start(self.total, &template);
        self.live.set_prefix("Running");
        self.started = true;
    }

    fn refresh_bar(&self) {
        self.live.set_position(self.completed);
        self.live.set_message(self.counts_message());
    }

    /// Build the colored "170 passed, 2 failed, 1 skipped" tail for the
    /// bar's `{msg}` slot. Segments with zero count are dropped.
    fn counts_message(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.passed > 0 {
            parts.push(format!("{}", format!("{} passed", self.passed).green()));
        }
        if self.failed > 0 {
            parts.push(format!(
                "{}",
                format!("{} failed", self.failed).red().bold()
            ));
        }
        if self.skipped > 0 {
            parts.push(format!("{}", format!("{} skipped", self.skipped).yellow()));
        }
        let sep = format!("{}", ", ".dimmed());
        parts.join(&sep)
    }
}

/// Right-aligned `   0.009s` form. Slow tests get yellow; very slow get
/// red (matches nextest's `--slow-timeout` highlight).
fn format_test_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    let raw = format!("{secs:>7.3}s");
    if d >= VERY_SLOW_TEST_THRESHOLD {
        format!("{}", raw.red())
    } else if d >= SLOW_TEST_THRESHOLD {
        format!("{}", raw.yellow())
    } else {
        raw
    }
}

/// Styled left column — file stem in cyan-bold to make the path
/// stand out (matching nextest's crate-name highlighting), groups in
/// cyan, ` > ` separators dimmed.
fn styled_left_label(test: &TestItem) -> String {
    let stem = test
        .file_path
        .as_deref()
        .and_then(Path::file_stem)
        .map_or_else(
            || test.module_path.clone(),
            |s| s.to_string_lossy().into_owned(),
        );
    if test.groups.is_empty() {
        format!("{}", stem.cyan().bold())
    } else {
        let sep = format!(" {} ", ">".dimmed());
        let groups_styled = test
            .groups
            .iter()
            .map(|g| format!("{}", g.cyan()))
            .collect::<Vec<_>>()
            .join(&sep);
        format!("{}{sep}{groups_styled}", stem.cyan().bold())
    }
}

impl<W: Write> Reporter for NextReporter<W> {
    fn on_run_start(&mut self, tests: &[TestItem]) {
        self.total = tests.len() as u64;
        self.completed = 0;
        self.passed = 0;
        self.failed = 0;
        self.skipped = 0;
        self.start = Instant::now();
        self.started = false;

        // Header lines go through the live area too; with no bar yet,
        // they're just plain writes (above where the bar will appear).
        let header = format!(
            "{} {}",
            self.subcommand_label.bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        );
        self.live.println(&mut self.writer, &header);
        self.live.println(&mut self.writer, "");
    }

    fn on_test_complete(&mut self, result: &TestResult) {
        // Bar is created lazily on the first completion so any setup-
        // time stderr noise (scheduler warnings, etc.) prints to a clean
        // terminal rather than fighting with a freshly-drawn bar.
        self.ensure_bar_started();

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
        let left_styled = styled_left_label(&result.test);
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

        let row = format!(
            "     {badge} [{}] {left_styled} {} {display}{suffix}",
            dur.dimmed(),
            "::".dimmed(),
        );
        self.live.println(&mut self.writer, &row);

        // Inline failure detail right after the row — keeps cause near
        // effect, matching nextest's behavior. Each line of the rendered
        // diagnostic goes through `live.println` so the bar is properly
        // cleared/redrawn around it.
        let detail = match &result.outcome {
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
            _ => String::new(),
        };
        if !detail.is_empty() {
            for line in detail.lines() {
                self.live.println(&mut self.writer, line);
            }
        }

        self.refresh_bar();
    }

    fn on_run_complete(&mut self, run_summary: &RunSummary) {
        self.live.finish_and_clear();
        summary::write_summary_with_hint(&mut self.writer, run_summary, self.watch_hint.as_deref());
    }

    fn set_subcommand_label(&mut self, label: &'static str) {
        self.subcommand_label = label;
    }

    fn set_watch_hint(&mut self, hint: Option<String>) {
        self.watch_hint = hint;
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
    fn writer_has_no_cursor_escapes() {
        // Bar lives in `LiveArea::hidden()` for `with_writer`; nothing
        // should ever emit cursor-control codes into the writer.
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
        let formatted = format_test_duration(Duration::from_millis(9));
        // Fast tests aren't styled — should be a literal padded number.
        assert_eq!(formatted, "  0.009s");
    }

    #[test]
    fn format_test_duration_seconds() {
        let formatted = format_test_duration(Duration::from_millis(800));
        assert_eq!(formatted, "  0.800s");
    }

    #[test]
    fn format_test_duration_slow_is_yellow() {
        let formatted = format_test_duration(Duration::from_millis(1500));
        assert!(
            formatted.contains("\x1b[33m") || formatted.contains("\x1b[1;33m"),
            "expected yellow ANSI escape, got {formatted:?}"
        );
    }

    #[test]
    fn format_test_duration_very_slow_is_red() {
        let formatted = format_test_duration(Duration::from_secs(7));
        assert!(
            formatted.contains("\x1b[31m") || formatted.contains("\x1b[1;31m"),
            "expected red ANSI escape, got {formatted:?}"
        );
    }
}
