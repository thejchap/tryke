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
    /// When `Some`, this item represents a doctest rather than a normal test.
    /// The value is the dotted attribute path to the object whose docstring
    /// should be tested (e.g. `"Foo.bar"`), or an empty string for the
    /// module-level docstring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doctest_object: Option<String>,
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
pub struct ChangedSelectionSummary {
    pub changed_files: usize,
    pub affected_tests: usize,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_selection: Option<ChangedSelectionSummary>,
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

/// The kind of issue detected during test discovery.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryWarningKind {
    /// File contains `importlib.import_module()` or `__import__()` calls.
    /// tryke cannot statically trace these imports, so the file is always
    /// included in `--changed` runs regardless of what actually changed.
    DynamicImports,
}

/// A non-fatal issue detected during test discovery that may degrade
/// the accuracy of selective re-runs or watch-mode module reloading.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveryWarning {
    pub file_path: PathBuf,
    pub kind: DiscoveryWarningKind,
    pub message: String,
}

/// Fixture granularity: whether a fixture's value is recomputed for every
/// test, or cached for the lifetime of its lexical scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixturePer {
    /// Fresh value per test; teardown after each test.
    #[default]
    Test,
    /// One value cached for the whole lexical scope (module or describe
    /// block); teardown after the last test in that scope.
    Scope,
}

impl FixturePer {
    /// Returns `true` for fixture kinds that force all tests in their scope
    /// onto the same worker (because they cache state across tests).
    #[must_use]
    pub fn constrains_scheduling(&self) -> bool {
        matches!(self, Self::Scope)
    }
}

/// A fixture discovered by static analysis.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HookItem {
    pub name: String,
    /// Dotted Python module path (e.g. ``tests.test_math``).
    pub module_path: String,
    pub per: FixturePer,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// Function names extracted from ``Depends()`` in parameter defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_number: Option<u32>,
}

/// The complete result of parsing a single Python source file.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParsedFile {
    pub tests: Vec<TestItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<HookItem>,
    /// Human-readable diagnostics produced during parsing. Currently used
    /// to report unsupported ``Depends(...)`` argument forms so users see
    /// a loud error instead of a silent no-op at resolution time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_warning_serializes() {
        let warning = DiscoveryWarning {
            file_path: PathBuf::from("tests/helpers/loader.py"),
            kind: DiscoveryWarningKind::DynamicImports,
            message: "dynamic imports detected".into(),
        };
        let json = serde_json::to_string(&warning).expect("serialize");
        assert!(json.contains("dynamic_imports"));
        assert!(json.contains("loader.py"));
    }

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

    #[test]
    fn fixture_per_serializes_to_snake_case() {
        let json = serde_json::to_string(&FixturePer::Test).expect("serialize");
        assert_eq!(json, r#""test""#);

        let json = serde_json::to_string(&FixturePer::Scope).expect("serialize");
        assert_eq!(json, r#""scope""#);
    }

    #[test]
    fn fixture_per_constrains_scheduling() {
        assert!(FixturePer::Scope.constrains_scheduling());
        assert!(!FixturePer::Test.constrains_scheduling());
    }

    #[test]
    fn hook_item_round_trips_through_serde() {
        let hook = HookItem {
            name: "setup_db".into(),
            module_path: "tests.test_setup".into(),
            per: FixturePer::Scope,
            groups: vec!["users".into()],
            depends_on: vec!["config".into()],
            line_number: Some(10),
        };
        let json = serde_json::to_string(&hook).expect("serialize");
        let back: HookItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(hook, back);
    }

    #[test]
    fn parsed_file_default_is_empty() {
        let pf = ParsedFile::default();
        assert!(pf.tests.is_empty());
        assert!(pf.hooks.is_empty());
    }

    #[test]
    fn parsed_file_round_trips_through_serde() {
        let pf = ParsedFile {
            tests: vec![TestItem {
                name: "test_foo".into(),
                module_path: "tests.test_foo".into(),
                ..Default::default()
            }],
            hooks: vec![HookItem {
                name: "db".into(),
                module_path: "tests.test_foo".into(),
                per: FixturePer::Scope,
                groups: vec![],
                depends_on: vec![],
                line_number: Some(5),
            }],
            errors: vec![],
        };
        let json = serde_json::to_string(&pf).expect("serialize");
        let back: ParsedFile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(pf, back);
    }
}
