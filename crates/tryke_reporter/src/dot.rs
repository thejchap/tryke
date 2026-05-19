use std::io;

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;

pub struct DotReporter<W: io::Write = io::Stdout> {
    writer: W,
    watch_hint: Option<String>,
    clear_armed: bool,
    clear_enabled: bool,
    /// See `TextReporter::header_pending` for rationale — defers the
    /// header until the first content event so an armed cycle keeps
    /// the previous run on screen through worker warmup.
    header_pending: bool,
}

impl DotReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
            watch_hint: None,
            clear_armed: false,
            clear_enabled: crate::clear::stdout_is_terminal(),
            header_pending: false,
        }
    }
}

impl Default for DotReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: io::Write> DotReporter<W> {
    pub fn with_writer(writer: W) -> Self {
        Self {
            writer,
            watch_hint: None,
            clear_armed: false,
            clear_enabled: false,
            header_pending: false,
        }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }

    fn flush_pending_clear(&mut self) {
        if self.clear_armed {
            if self.clear_enabled {
                crate::clear::clear_terminal();
            }
            self.clear_armed = false;
        }
    }

    fn write_header(&mut self) {
        let _ = writeln!(
            self.writer,
            "{} {}",
            "tryke test".bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        );
        let _ = writeln!(self.writer);
    }

    fn flush_pending_header(&mut self) {
        if self.header_pending {
            self.flush_pending_clear();
            self.write_header();
            self.header_pending = false;
        }
    }
}

impl<W: io::Write> Reporter for DotReporter<W> {
    fn on_run_start(&mut self, _tests: &[TestItem]) {
        if self.clear_armed {
            self.header_pending = true;
        } else {
            self.write_header();
        }
    }

    fn on_test_complete(&mut self, result: &TestResult) {
        self.flush_pending_header();
        let ch = match &result.outcome {
            TestOutcome::Passed => ".".green().to_string(),
            TestOutcome::Failed { .. } => "F".red().to_string(),
            TestOutcome::Skipped { .. } => "s".yellow().dimmed().to_string(),
            TestOutcome::Error { .. } => "E".red().to_string(),
            TestOutcome::XFailed { .. } => "x".yellow().dimmed().to_string(),
            TestOutcome::XPassed => "X".red().to_string(),
            TestOutcome::Todo { .. } => "T".cyan().dimmed().to_string(),
        };
        let _ = write!(self.writer, "{ch}");
        let _ = self.writer.flush();
    }

    fn on_run_complete(&mut self, summary: &RunSummary) {
        self.flush_pending_header();
        let _ = writeln!(self.writer);
        crate::summary::write_summary_with_hint(
            &mut self.writer,
            summary,
            self.watch_hint.as_deref(),
        );
    }

    fn on_collect_complete(&mut self, tests: &[TestItem]) {
        crate::summary::write_collect_list(&mut self.writer, "tryke test", tests);
    }

    fn set_watch_hint(&mut self, hint: Option<String>) {
        self.watch_hint = hint;
    }

    fn arm_clear(&mut self) {
        self.clear_armed = true;
    }

    fn on_scheduler_warning(&mut self, message: &str) {
        self.flush_pending_header();
        let _ = writeln!(self.writer, "{} {message}", "warning:".yellow().bold());
    }

    fn on_watch_idle(&mut self, info: &crate::reporter::WatchIdleInfo<'_>) {
        self.flush_pending_clear();
        self.header_pending = false;
        self.write_header();
        crate::summary::write_idle_summary(&mut self.writer, info);
    }

    fn on_watch_results_cleared(&mut self, info: &crate::reporter::WatchIdleInfo<'_>) {
        self.clear_armed = true;
        self.flush_pending_clear();
        self.header_pending = false;
        self.write_header();
        crate::summary::write_cleared_summary(&mut self.writer, info);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tryke_types::{TestItem, TestOutcome, TestResult};

    use super::*;

    fn reporter() -> DotReporter<Vec<u8>> {
        DotReporter::with_writer(Vec::new())
    }

    fn output(r: &DotReporter<Vec<u8>>) -> String {
        String::from_utf8_lossy(&r.writer).into_owned()
    }

    fn test_item(name: &str) -> TestItem {
        TestItem {
            name: name.into(),
            module_path: "tests.mod".into(),
            ..Default::default()
        }
    }

    #[test]
    fn on_test_complete_passed() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item("t"),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        assert!(output(&r).contains('.'));
    }

    #[test]
    fn on_test_complete_failed() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item("t"),
            outcome: TestOutcome::Failed {
                message: "bad".into(),
                traceback: None,
                assertions: vec![],
                executed_lines: vec![],
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        assert!(output(&r).contains('F'));
    }

    #[test]
    fn on_test_complete_skipped() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item("t"),
            outcome: TestOutcome::Skipped { reason: None },
            duration: Duration::from_millis(0),
            stdout: String::new(),
            stderr: String::new(),
        });
        assert!(output(&r).contains('s'));
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
        assert!(out.contains("1 failed"));
        assert!(out.contains("3 passed"));
        assert!(out.contains("2 skipped"));
        assert!(out.contains("(6)"));
    }

    #[test]
    fn collect_only_lists_tests_with_header_and_count() {
        let mut r = reporter();
        let tests = vec![
            TestItem {
                name: "test_add".into(),
                module_path: "tests.math".into(),
                file_path: Some(std::path::PathBuf::from("tests/math.py")),
                ..Default::default()
            },
            TestItem {
                name: "test_sub".into(),
                module_path: "tests.math".into(),
                file_path: Some(std::path::PathBuf::from("tests/math.py")),
                ..Default::default()
            },
        ];
        r.on_collect_complete(&tests);
        let out = output(&r);
        assert!(out.contains("tryke test"));
        assert!(out.contains("tests/math.py:"));
        assert!(out.contains("test_add"));
        assert!(out.contains("test_sub"));
        assert!(out.contains("2 tests collected."));
    }

    #[test]
    fn full_lifecycle() {
        let mut r = reporter();
        let tests = vec![test_item("test_one")];

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
        assert!(out.contains('.'));
        assert!(out.contains("PASS"));
        assert!(out.contains("1 passed"));
    }
}
