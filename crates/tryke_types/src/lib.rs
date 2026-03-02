use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExpectedAssertion {
    pub subject: String,
    pub matcher: String,
    pub negated: bool,
    pub args: Vec<String>,
    pub line: u32,
    pub label: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Assertion {
    pub expression: String,
    pub file: Option<String>,
    pub line: usize,
    pub span_offset: usize,
    pub span_length: usize,
    pub expected: String,
    pub received: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestItem {
    pub name: String,
    pub module_path: String,
    pub file_path: Option<PathBuf>,
    pub line_number: Option<u32>,
    pub display_name: Option<String>,
    pub expected_assertions: Vec<ExpectedAssertion>,
}

impl TestItem {
    #[must_use]
    pub fn id(&self) -> String {
        match &self.file_path {
            Some(path) => format!("{}::{}", path.display(), self.name),
            None => format!("{}::{}", self.module_path, self.name),
        }
    }
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
    pub test: TestItem,
    pub outcome: TestOutcome,
    pub duration: Duration,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunSummary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration: Duration,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileDiscovery {
    pub file_path: PathBuf,
    pub tests: Vec<TestItem>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveryResult {
    pub files: Vec<FileDiscovery>,
    pub errors: Vec<DiscoveryError>,
    pub duration: Duration,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveryError {
    pub file_path: PathBuf,
    pub message: String,
    pub line_number: Option<u32>,
}
