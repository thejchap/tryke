use std::io::{self, Write};

use tryke_types::{DiscoveryError, RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;

/// <https://ghostty.org/docs/install/release-notes/1-2-0#graphical-progress-bars>
/// <https://conemu.github.io/en/AnsiEscapeCodes.html#ConEmu_specific_OSC>
pub struct ProgressReporter<R: Reporter> {
    inner: R,
    total: usize,
    completed: usize,
    has_failure: bool,
}

fn emit_osc(state: u8, value: u8) {
    let stderr = io::stderr();
    let mut handle = stderr.lock();
    let _ = write!(handle, "\x1b]9;4;{state};{value}\x1b\\");
    let _ = handle.flush();
}

#[must_use]
pub fn supports_progress() -> bool {
    use std::env;
    use std::io::IsTerminal;

    if !io::stderr().is_terminal() {
        return false;
    }

    if env::var_os("WT_SESSION").is_some() {
        return true;
    }
    if env::var("ConEmuANSI").ok().as_deref() == Some("ON") {
        return true;
    }

    match env::var("TERM_PROGRAM").ok().as_deref() {
        Some("ghostty" | "WezTerm") => true,
        Some("iTerm.app") => env::var("TERM_FEATURES")
            .ok()
            .is_some_and(|f| f.split(',').any(|tok| tok.trim() == "P")),
        _ => false,
    }
}

impl<R: Reporter> ProgressReporter<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            total: 0,
            completed: 0,
            has_failure: false,
        }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Reporter> Reporter for ProgressReporter<R> {
    fn on_run_start(&mut self, tests: &[TestItem]) {
        self.total = tests.len();
        self.completed = 0;
        self.has_failure = false;
        if self.total > 0 {
            emit_osc(1, 0);
        }
        self.inner.on_run_start(tests);
    }

    fn on_test_complete(&mut self, result: &TestResult) {
        self.completed += 1;
        if matches!(
            result.outcome,
            TestOutcome::Failed { .. } | TestOutcome::Error { .. } | TestOutcome::XPassed
        ) {
            self.has_failure = true;
        }
        // value is clamped to 0..=100, safe to truncate
        let pct = u8::try_from((self.completed * 100 / self.total).min(100)).unwrap_or(100);
        emit_osc(1, pct);
        self.inner.on_test_complete(result);
    }

    fn on_run_complete(&mut self, summary: &RunSummary) {
        if self.has_failure {
            emit_osc(2, 0);
        } else {
            emit_osc(0, 0);
        }
        self.inner.on_run_complete(summary);
    }

    fn on_collect_complete(&mut self, tests: &[TestItem]) {
        self.inner.on_collect_complete(tests);
    }

    fn on_discovery_error(&mut self, error: &DiscoveryError) {
        self.inner.on_discovery_error(error);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

    use super::*;

    struct RecordingReporter {
        started: bool,
        results: Vec<String>,
        completed: bool,
    }

    impl RecordingReporter {
        fn new() -> Self {
            Self {
                started: false,
                results: Vec::new(),
                completed: false,
            }
        }
    }

    impl Reporter for RecordingReporter {
        fn on_run_start(&mut self, _tests: &[TestItem]) {
            self.started = true;
        }

        fn on_test_complete(&mut self, result: &TestResult) {
            self.results.push(result.test.name.clone());
        }

        fn on_run_complete(&mut self, _summary: &RunSummary) {
            self.completed = true;
        }
    }

    fn test_item(name: &str) -> TestItem {
        TestItem {
            name: name.into(),
            module_path: "tests.mod".into(),
            ..Default::default()
        }
    }

    #[test]
    fn delegates_to_inner_reporter() {
        let inner = RecordingReporter::new();
        let mut reporter = ProgressReporter::new(inner);
        let tests = vec![test_item("test_one"), test_item("test_two")];

        reporter.on_run_start(&tests);
        assert!(reporter.inner.started);
        assert_eq!(reporter.total, 2);
        assert_eq!(reporter.completed, 0);

        reporter.on_test_complete(&TestResult {
            test: tests[0].clone(),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(10),
            stdout: String::new(),
            stderr: String::new(),
        });
        assert_eq!(reporter.completed, 1);
        assert!(!reporter.has_failure);

        reporter.on_test_complete(&TestResult {
            test: tests[1].clone(),
            outcome: TestOutcome::Failed {
                message: "bad".into(),
                traceback: None,
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });
        assert_eq!(reporter.completed, 2);
        assert!(reporter.has_failure);

        reporter.on_run_complete(&RunSummary {
            passed: 1,
            failed: 1,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(15),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(reporter.inner.completed);
        assert_eq!(reporter.inner.results.len(), 2);
    }

    #[test]
    fn zero_tests_does_not_panic() {
        let inner = RecordingReporter::new();
        let mut reporter = ProgressReporter::new(inner);

        reporter.on_run_start(&[]);
        assert_eq!(reporter.total, 0);

        reporter.on_run_complete(&RunSummary {
            passed: 0,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(0),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(reporter.inner.completed);
    }

    #[test]
    fn tracks_error_as_failure() {
        let inner = RecordingReporter::new();
        let mut reporter = ProgressReporter::new(inner);
        let tests = vec![test_item("test_err")];

        reporter.on_run_start(&tests);
        reporter.on_test_complete(&TestResult {
            test: tests[0].clone(),
            outcome: TestOutcome::Error {
                message: "boom".into(),
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        assert!(reporter.has_failure);
    }

    #[test]
    fn into_inner_returns_inner() {
        let inner = RecordingReporter::new();
        let reporter = ProgressReporter::new(inner);
        let recovered = reporter.into_inner();
        assert!(!recovered.started);
    }
}
