use std::io;

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem, TestOutcome, TestResult};

use crate::Reporter;

pub struct DotReporter<W: io::Write = io::Stdout> {
    writer: W,
}

impl DotReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
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
        Self { writer }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }
}

impl<W: io::Write> Reporter for DotReporter<W> {
    fn on_run_start(&mut self, _tests: &[TestItem]) {
        let _ = writeln!(
            self.writer,
            "{} {}",
            "tryke test".bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        );
        let _ = writeln!(self.writer);
    }

    fn on_test_complete(&mut self, result: &TestResult) {
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
        let _ = writeln!(self.writer);
        crate::summary::write_summary(&mut self.writer, summary);
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
        });
        let out = output(&r);
        assert!(out.contains("FAIL"));
        assert!(out.contains("1 failed"));
        assert!(out.contains("3 passed"));
        assert!(out.contains("2 skipped"));
        assert!(out.contains("(6)"));
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
        });

        let out = output(&r);
        assert!(out.contains("tryke test"));
        assert!(out.contains('.'));
        assert!(out.contains("PASS"));
        assert!(out.contains("1 passed"));
    }
}
