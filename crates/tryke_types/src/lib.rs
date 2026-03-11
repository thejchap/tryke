pub mod filter;

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Convert a file path to a Python module name relative to `root`.
/// e.g. `/project/tests/test_math.py` → `"tests.test_math"`
///
/// Returns `None` if `path` is not under `root` or has no components.
#[must_use]
pub fn path_to_module(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let without_ext = relative.with_extension("");
    let parts: Vec<String> = without_ext
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("."))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExpectedAssertion {
    pub subject: String,
    pub matcher: String,
    pub negated: bool,
    pub args: Vec<String>,
    pub line: u32,
    pub label: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Assertion {
    pub expression: String,
    pub file: Option<String>,
    pub line: usize,
    pub span_offset: usize,
    pub span_length: usize,
    pub expected: String,
    pub received: String,
    /// Byte offset and length of the matcher argument in `expression`,
    /// e.g. the `2` in `expect(x).to_equal(2)`. `None` for no-arg matchers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_arg_span: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TestItem {
    pub name: String,
    pub module_path: String,
    pub file_path: Option<PathBuf>,
    pub line_number: Option<u32>,
    pub display_name: Option<String>,
    pub expected_assertions: Vec<ExpectedAssertion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xfail: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "detail")]
pub enum TestOutcome {
    Passed,
    Failed {
        message: String,
        #[serde(default)]
        traceback: Option<String>,
        assertions: Vec<Assertion>,
    },
    Skipped {
        reason: Option<String>,
    },
    Error {
        message: String,
    },
    XFailed {
        reason: Option<String>,
    },
    XPassed,
    Todo {
        description: Option<String>,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestResult {
    pub test: TestItem,
    pub outcome: TestOutcome,
    pub duration: Duration,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunSummary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    #[serde(default)]
    pub errors: usize,
    #[serde(default)]
    pub xfailed: usize,
    #[serde(default)]
    pub todo: usize,
    pub duration: Duration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_duration: Option<Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_duration: Option<Duration>,
    #[serde(default)]
    pub file_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_to_module_basic() {
        let root = PathBuf::from("/project");
        let path = PathBuf::from("/project/tests/test_math.py");
        assert_eq!(
            path_to_module(&root, &path),
            Some("tests.test_math".to_string())
        );
    }

    #[test]
    fn path_to_module_top_level() {
        let root = PathBuf::from("/project");
        let path = PathBuf::from("/project/test_foo.py");
        assert_eq!(path_to_module(&root, &path), Some("test_foo".to_string()));
    }

    #[test]
    fn path_to_module_not_under_root() {
        let root = PathBuf::from("/project");
        let path = PathBuf::from("/other/test_foo.py");
        assert_eq!(path_to_module(&root, &path), None);
    }

    #[test]
    fn path_to_module_root_itself() {
        let root = PathBuf::from("/project");
        let path = PathBuf::from("/project");
        assert_eq!(path_to_module(&root, &path), None);
    }
}
