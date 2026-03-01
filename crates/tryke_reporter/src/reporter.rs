use tryke_types::{RunSummary, TestCase, TestResult};

pub trait Reporter {
    fn on_run_start(&mut self, tests: &[TestCase]);
    fn on_test_complete(&mut self, result: &TestResult);
    fn on_run_complete(&mut self, summary: &RunSummary);
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
        fn on_run_start(&mut self, _tests: &[TestCase]) {
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
            TestCase {
                name: "test_add".into(),
                module: "math".into(),
                file: None,
            },
            TestCase {
                name: "test_sub".into(),
                module: "math".into(),
                file: None,
            },
        ];

        reporter.on_run_start(&tests);
        assert!(reporter.started);

        reporter.on_test_complete(&TestResult {
            test: tests[0].clone(),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(10),
        });

        reporter.on_test_complete(&TestResult {
            test: tests[1].clone(),
            outcome: TestOutcome::Failed {
                message: "expected 1, got 2".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
        });

        assert_eq!(reporter.results.len(), 2);

        reporter.on_run_complete(&RunSummary {
            passed: 1,
            failed: 1,
            skipped: 0,
            duration: Duration::from_millis(15),
        });

        let summary = reporter.summary.as_ref().expect("summary should be set");
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 0);
    }
}
