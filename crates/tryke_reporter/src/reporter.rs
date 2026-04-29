use tryke_types::{DiscoveryError, DiscoveryWarning, RunSummary, TestItem, TestResult};

pub trait Reporter {
    fn on_run_start(&mut self, tests: &[TestItem]);
    fn on_test_complete(&mut self, result: &TestResult);
    fn on_run_complete(&mut self, summary: &RunSummary);
    fn on_collect_complete(&mut self, _tests: &[TestItem]) {}
    fn on_discovery_error(&mut self, _error: &DiscoveryError) {}
    /// Called once per file when dynamic imports are detected during discovery.
    /// Implementations should surface this to the user so they understand why
    /// those files are always included in `--changed` runs.
    fn on_discovery_warning(&mut self, _warning: &DiscoveryWarning) {}
    /// Lets the CLI tell the reporter which subcommand invoked it, so run
    /// headers can read "tryke test --watch" instead of the generic "tryke test".
    fn set_subcommand_label(&mut self, _label: &'static str) {}
    /// In watch mode, sets a short trailing hint shown next to the
    /// pass/fail badge in the run summary (e.g. "Waiting for file
    /// changes..."). Reporters that don't render the summary line can
    /// ignore this.
    fn set_watch_hint(&mut self, _hint: Option<String>) {}
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tryke_types::TestOutcome;

    use super::*;

    struct RecordingReporter {
        started: bool,
        results: Vec<TestResult>,
        summary: Option<RunSummary>,
    }

    impl RecordingReporter {
        fn new() -> Self {
            Self {
                started: false,
                results: Vec::new(),
                summary: None,
            }
        }
    }

    impl Reporter for RecordingReporter {
        fn on_run_start(&mut self, _tests: &[TestItem]) {
            self.started = true;
        }

        fn on_test_complete(&mut self, result: &TestResult) {
            self.results.push(result.clone());
        }

        fn on_run_complete(&mut self, summary: &RunSummary) {
            self.summary = Some(summary.clone());
        }
    }

    #[test]
    fn reporter_lifecycle() {
        let mut reporter = RecordingReporter::new();

        let tests = vec![
            TestItem {
                name: "test_add".into(),
                module_path: "tests.math".into(),
                ..Default::default()
            },
            TestItem {
                name: "test_sub".into(),
                module_path: "tests.math".into(),
                ..Default::default()
            },
        ];

        reporter.on_run_start(&tests);
        assert!(reporter.started);

        reporter.on_test_complete(&TestResult {
            test: tests[0].clone(),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(10),
            stdout: String::new(),
            stderr: String::new(),
        });

        reporter.on_test_complete(&TestResult {
            test: tests[1].clone(),
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

        assert_eq!(reporter.results.len(), 2);

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

        let summary = reporter.summary.as_ref().expect("summary should be set");
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 0);
    }
}
