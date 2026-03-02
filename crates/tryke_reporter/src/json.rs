use std::io;

use serde::Serialize;
use tryke_types::{RunSummary, TestItem, TestResult};

use crate::Reporter;

pub struct JSONReporter<W: io::Write = io::Stdout> {
    writer: W,
}

impl JSONReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
        }
    }
}

impl Default for JSONReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: io::Write> JSONReporter<W> {
    pub fn with_writer(writer: W) -> Self {
        Self { writer }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }

    fn write_event<T: Serialize>(&mut self, event: &T) {
        // ignore write errors to match typical reporter behavior
        let _ = serde_json::to_writer(&mut self.writer, event)
            .map_err(io::Error::from)
            .and_then(|()| self.writer.write_all(b"\n"));
    }
}

#[derive(Serialize)]
struct RunStartEvent<'a> {
    event: &'static str,
    tests: &'a [TestItem],
}

#[derive(Serialize)]
struct TestCompleteEvent<'a> {
    event: &'static str,
    result: &'a TestResult,
}

#[derive(Serialize)]
struct RunCompleteEvent<'a> {
    event: &'static str,
    summary: &'a RunSummary,
}

#[derive(Serialize)]
struct CollectCompleteEvent<'a> {
    event: &'static str,
    tests: &'a [TestItem],
}

impl<W: io::Write> Reporter for JSONReporter<W> {
    fn on_run_start(&mut self, tests: &[TestItem]) {
        self.write_event(&RunStartEvent {
            event: "run_start",
            tests,
        });
    }

    fn on_test_complete(&mut self, result: &TestResult) {
        self.write_event(&TestCompleteEvent {
            event: "test_complete",
            result,
        });
    }

    fn on_run_complete(&mut self, summary: &RunSummary) {
        self.write_event(&RunCompleteEvent {
            event: "run_complete",
            summary,
        });
    }

    fn on_collect_complete(&mut self, tests: &[TestItem]) {
        self.write_event(&CollectCompleteEvent {
            event: "collect_complete",
            tests,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tryke_types::{Assertion, TestOutcome};

    use super::*;

    fn reporter() -> JSONReporter<Vec<u8>> {
        JSONReporter::with_writer(Vec::new())
    }

    fn output_lines(reporter: &JSONReporter<Vec<u8>>) -> Vec<serde_json::Value> {
        let output = String::from_utf8_lossy(&reporter.writer);
        output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).expect("valid json"))
            .collect()
    }

    #[test]
    fn emits_run_start() {
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
        let lines = output_lines(&r);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["event"], "run_start");
        assert_eq!(lines[0]["tests"][0]["name"], "test_one");
        assert_eq!(lines[0]["tests"][0]["module_path"], "tests.mod_a");
    }

    #[test]
    fn emits_test_complete_passed() {
        let mut r = reporter();
        let result = TestResult {
            test: TestItem {
                name: "test_add".into(),
                module_path: "tests.math".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(42),
            stdout: String::new(),
            stderr: String::new(),
        };

        r.on_test_complete(&result);
        let lines = output_lines(&r);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["event"], "test_complete");
        assert_eq!(lines[0]["result"]["test"]["name"], "test_add");
        assert_eq!(lines[0]["result"]["outcome"]["status"], "passed");
    }

    #[test]
    fn emits_test_complete_failed() {
        let mut r = reporter();
        let result = TestResult {
            test: TestItem {
                name: "test_sub".into(),
                module_path: "tests.math".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
            outcome: TestOutcome::Failed {
                message: "expected 1, got 2".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        };

        r.on_test_complete(&result);
        let lines = output_lines(&r);

        assert_eq!(lines[0]["result"]["outcome"]["status"], "failed");
        assert_eq!(
            lines[0]["result"]["outcome"]["detail"]["message"],
            "expected 1, got 2"
        );
    }

    #[test]
    fn emits_test_complete_skipped() {
        let mut r = reporter();
        let result = TestResult {
            test: TestItem {
                name: "test_skip".into(),
                module_path: "tests.misc".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
            outcome: TestOutcome::Skipped {
                reason: Some("not implemented".into()),
            },
            duration: Duration::from_millis(0),
            stdout: String::new(),
            stderr: String::new(),
        };

        r.on_test_complete(&result);
        let lines = output_lines(&r);

        assert_eq!(lines[0]["result"]["outcome"]["status"], "skipped");
        assert_eq!(
            lines[0]["result"]["outcome"]["detail"]["reason"],
            "not implemented"
        );
    }

    #[test]
    fn emits_run_complete() {
        let mut r = reporter();
        let summary = RunSummary {
            passed: 5,
            failed: 1,
            skipped: 2,
            duration: Duration::from_millis(100),
        };

        r.on_run_complete(&summary);
        let lines = output_lines(&r);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["event"], "run_complete");
        assert_eq!(lines[0]["summary"]["passed"], 5);
        assert_eq!(lines[0]["summary"]["failed"], 1);
        assert_eq!(lines[0]["summary"]["skipped"], 2);
    }

    #[test]
    fn full_lifecycle_produces_three_lines() {
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

        r.on_test_complete(&TestResult {
            test: tests[0].clone(),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(10),
            stdout: String::new(),
            stderr: String::new(),
        });

        r.on_test_complete(&TestResult {
            test: tests[1].clone(),
            outcome: TestOutcome::Failed {
                message: "boom".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });

        r.on_run_complete(&RunSummary {
            passed: 1,
            failed: 1,
            skipped: 0,
            duration: Duration::from_millis(15),
        });

        let lines = output_lines(&r);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0]["event"], "run_start");
        assert_eq!(lines[1]["event"], "test_complete");
        assert_eq!(lines[2]["event"], "test_complete");
        assert_eq!(lines[3]["event"], "run_complete");
    }

    #[test]
    fn emits_collect_complete() {
        let mut r = reporter();
        let tests = vec![
            TestItem {
                name: "test_add".into(),
                module_path: "tests.math".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
            TestItem {
                name: "test_sub".into(),
                module_path: "tests.math".into(),
                file_path: None,
                line_number: None,
                display_name: None,
                expected_assertions: vec![],
            },
        ];
        r.on_collect_complete(&tests);
        let lines = output_lines(&r);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["event"], "collect_complete");
        assert_eq!(lines[0]["tests"][0]["name"], "test_add");
        assert_eq!(lines[0]["tests"][1]["name"], "test_sub");
    }

    #[test]
    fn failed_with_assertions_includes_data() {
        let mut r = reporter();
        let result = TestResult {
            test: TestItem {
                name: "test_add".into(),
                module_path: "tests.math".into(),
                file_path: Some("tests/math.py".into()),
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
        };

        r.on_test_complete(&result);
        let lines = output_lines(&r);

        let detail = &lines[0]["result"]["outcome"]["detail"];
        assert_eq!(detail["assertions"][0]["expression"], "assert_eq!(a, 2)");
        assert_eq!(detail["assertions"][0]["expected"], "2");
        assert_eq!(detail["assertions"][0]["received"], "3");
        assert_eq!(detail["assertions"][0]["line"], 10);
    }
}
