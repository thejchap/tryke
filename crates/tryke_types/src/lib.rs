use std::time::Duration;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Assertion {
    pub expression: String,
    pub line: usize,
    pub span_offset: usize,
    pub span_length: usize,
    pub expected: String,
    pub received: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestCase {
    pub name: String,
    pub module: String,
    pub file: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "detail")]
pub enum TestOutcome {
    Passed,
    Failed {
        message: String,
        assertions: Vec<Assertion>,
    },
    Skipped {
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestResult {
    pub test: TestCase,
    pub outcome: TestOutcome,
    pub duration: Duration,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunSummary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration: Duration,
}
