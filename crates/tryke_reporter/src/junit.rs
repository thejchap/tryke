use std::io;

use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;

pub struct JUnitReporter<W: io::Write = io::Stdout> {
    writer: W,
    results: Vec<TestResult>,
}

impl JUnitReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
            results: Vec::new(),
        }
    }
}

impl Default for JUnitReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: io::Write> JUnitReporter<W> {
    pub fn with_writer(writer: W) -> Self {
        Self {
            writer,
            results: Vec::new(),
        }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

impl<W: io::Write> Reporter for JUnitReporter<W> {
    fn on_run_start(&mut self, _tests: &[TestItem]) {}

    fn on_test_complete(&mut self, result: &TestResult) {
        self.results.push(result.clone());
    }

    fn on_run_complete(&mut self, summary: &RunSummary) {
        let total = summary.passed + summary.failed + summary.skipped;
        let suite_time = summary.duration.as_secs_f64();

        let _ = writeln!(self.writer, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        let _ = writeln!(
            self.writer,
            r#"<testsuite name="tryke" tests="{}" failures="{}" skipped="{}" time="{:.3}">"#,
            total, summary.failed, summary.skipped, suite_time
        );

        for result in &self.results {
            let name = xml_escape(
                result
                    .test
                    .display_name
                    .as_deref()
                    .unwrap_or(&result.test.name),
            );
            let classname = xml_escape(&result.test.module_path);
            let time = result.duration.as_secs_f64();

            match &result.outcome {
                TestOutcome::Passed => {
                    let _ = writeln!(
                        self.writer,
                        r#"  <testcase name="{name}" classname="{classname}" time="{time:.3}"/>"#,
                    );
                }
                TestOutcome::Failed { message, .. } => {
                    let msg = xml_escape(message);
                    let _ = writeln!(
                        self.writer,
                        r#"  <testcase name="{name}" classname="{classname}" time="{time:.3}">"#,
                    );
                    let _ = writeln!(self.writer, r#"    <failure message="{msg}"/>"#);
                    let _ = writeln!(self.writer, "  </testcase>");
                }
                TestOutcome::Skipped { .. } => {
                    let _ = writeln!(
                        self.writer,
                        r#"  <testcase name="{name}" classname="{classname}" time="{time:.3}">"#,
                    );
                    let _ = writeln!(self.writer, "    <skipped/>");
                    let _ = writeln!(self.writer, "  </testcase>");
                }
            }
        }

        let _ = writeln!(self.writer, "</testsuite>");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tryke_types::{TestItem, TestOutcome, TestResult};

    use super::*;

    fn reporter() -> JUnitReporter<Vec<u8>> {
        JUnitReporter::with_writer(Vec::new())
    }

    fn output(r: &JUnitReporter<Vec<u8>>) -> String {
        String::from_utf8_lossy(&r.writer).into_owned()
    }

    fn test_item(name: &str, module_path: &str) -> TestItem {
        TestItem {
            name: name.into(),
            module_path: module_path.into(),
            file_path: None,
            line_number: None,
            display_name: None,
            expected_assertions: vec![],
        }
    }

    fn run_suite(r: &mut JUnitReporter<Vec<u8>>) {
        r.on_test_complete(&TestResult {
            test: test_item("test_add", "tests.math"),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(12),
            stdout: String::new(),
            stderr: String::new(),
        });
        r.on_test_complete(&TestResult {
            test: test_item("test_sub", "tests.math"),
            outcome: TestOutcome::Failed {
                message: "assertion failed: 3 - 1 == 3".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        });
        r.on_test_complete(&TestResult {
            test: test_item("test_skip", "tests.parser"),
            outcome: TestOutcome::Skipped { reason: None },
            duration: Duration::from_millis(0),
            stdout: String::new(),
            stderr: String::new(),
        });
        r.on_run_complete(&RunSummary {
            passed: 1,
            failed: 1,
            skipped: 1,
            duration: Duration::from_millis(15),
        });
    }

    #[test]
    fn emits_xml_header() {
        let mut r = reporter();
        run_suite(&mut r);
        assert!(output(&r).starts_with("<?xml"));
    }

    #[test]
    fn testsuite_attributes() {
        let mut r = reporter();
        run_suite(&mut r);
        let out = output(&r);
        assert!(out.contains(r#"tests="3""#));
        assert!(out.contains(r#"failures="1""#));
        assert!(out.contains(r#"skipped="1""#));
    }

    #[test]
    fn passed_testcase_is_self_closing() {
        let mut r = reporter();
        run_suite(&mut r);
        let out = output(&r);
        assert!(out.contains(r#"<testcase name="test_add" classname="tests.math""#));
        assert!(out.contains(r#"name="test_add" classname="tests.math" time="0.012"/>"#));
    }

    #[test]
    fn failed_testcase_has_failure_element() {
        let mut r = reporter();
        run_suite(&mut r);
        let out = output(&r);
        assert!(out.contains(r#"<failure message="assertion failed: 3 - 1 == 3"/>"#));
    }

    #[test]
    fn skipped_testcase_has_skipped_element() {
        let mut r = reporter();
        run_suite(&mut r);
        assert!(output(&r).contains("<skipped/>"));
    }

    #[test]
    fn xml_escape_in_failure_message() {
        let mut r = reporter();
        r.on_test_complete(&TestResult {
            test: test_item("test_amp", "tests.misc"),
            outcome: TestOutcome::Failed {
                message: "a & b".into(),
                assertions: vec![],
            },
            duration: Duration::from_millis(1),
            stdout: String::new(),
            stderr: String::new(),
        });
        r.on_run_complete(&RunSummary {
            passed: 0,
            failed: 1,
            skipped: 0,
            duration: Duration::from_millis(1),
        });
        assert!(output(&r).contains("a &amp; b"));
    }
}
