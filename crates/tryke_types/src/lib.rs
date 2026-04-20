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
    /// For tests declared with `@test.cases(...)`, the label of the specific
    /// case this item represents (e.g. `"zero"` for `@test.cases(zero=...)`).
    /// `None` for plain `@test` functions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_label: Option<String>,
    /// Zero-based index of this case within its parent function, preserving
    /// declaration order. `None` when `case_label` is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_index: Option<u32>,
}

impl TestItem {
    #[must_use]
    pub fn id(&self) -> String {
        let base = match &self.file_path {
            Some(path) => format!("{}::{}", path.display(), self.name),
            None => format!("{}::{}", self.module_path, self.name),
        };
        match &self.case_label {
            Some(label) => format!("{base}[{label}]"),
            None => base,
        }
    }

    /// Human-readable label for reporters.
    ///
    /// Returns the `display_name` override if present, otherwise the bare
    /// function name. For `@test.cases(...)` items, appends `[case_label]`
    /// so every row in the output is disambiguated.
    #[must_use]
    pub fn display_label(&self) -> String {
        let base = self.display_name.as_deref().unwrap_or(&self.name);
        match &self.case_label {
            Some(label) => format!("{base}[{label}]"),
            None => base.to_owned(),
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
        /// Line numbers of every `expect()` call that actually executed,
        /// in order. Reporters use this to distinguish "ran and passed"
        /// from "never ran because an earlier statement raised".
        #[serde(default)]
        executed_lines: Vec<u32>,
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
    /// File contains `if __TRYKE_TESTING__:` with an `elif` or `else` branch.
    /// Discovery does not descend into guards with alternative branches, so
    /// any tests inside would be silently dropped; surface it instead.
    TestingGuardHasElseBranch,
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
    /// 1-indexed source lines where `if __TRYKE_TESTING__:` has an
    /// `elif`/`else` clause. These shapes get silently ignored by discovery
    /// (see `TestingGuardHasElseBranch` warning) so we record them here and
    /// surface them to the user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub testing_guard_else_lines: Vec<u32>,
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
    fn test_item_id_plain() {
        let item = TestItem {
            name: "test_square".into(),
            module_path: "tests.test_math".into(),
            file_path: Some(PathBuf::from("tests/test_math.py")),
            ..Default::default()
        };
        assert_eq!(item.id(), "tests/test_math.py::test_square");
    }

    #[test]
    fn test_item_id_with_case_label() {
        let item = TestItem {
            name: "square".into(),
            module_path: "tests.test_math".into(),
            file_path: Some(PathBuf::from("tests/test_math.py")),
            case_label: Some("zero".into()),
            case_index: Some(0),
            ..Default::default()
        };
        assert_eq!(item.id(), "tests/test_math.py::square[zero]");
    }

    #[test]
    fn test_item_case_label_round_trips_through_serde() {
        let item = TestItem {
            name: "square".into(),
            module_path: "tests.test_math".into(),
            file_path: Some(PathBuf::from("tests/test_math.py")),
            case_label: Some("ten".into()),
            case_index: Some(2),
            ..Default::default()
        };
        let json = serde_json::to_string(&item).expect("serialize");
        assert!(json.contains(r#""case_label":"ten""#), "json: {json}");
        assert!(json.contains(r#""case_index":2"#), "json: {json}");
        let back: TestItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.case_label.as_deref(), Some("ten"));
        assert_eq!(back.case_index, Some(2));
    }

    #[test]
    fn test_item_display_label_plain() {
        let item = TestItem {
            name: "test_square".into(),
            module_path: "tests.m".into(),
            ..Default::default()
        };
        assert_eq!(item.display_label(), "test_square");
    }

    #[test]
    fn test_item_display_label_prefers_display_name() {
        let item = TestItem {
            name: "test_square".into(),
            module_path: "tests.m".into(),
            display_name: Some("squares a number".into()),
            ..Default::default()
        };
        assert_eq!(item.display_label(), "squares a number");
    }

    #[test]
    fn test_item_display_label_with_case_label() {
        let item = TestItem {
            name: "square".into(),
            module_path: "tests.m".into(),
            case_label: Some("zero".into()),
            case_index: Some(0),
            ..Default::default()
        };
        assert_eq!(item.display_label(), "square[zero]");
    }

    #[test]
    fn test_item_display_label_combines_display_name_and_case_label() {
        let item = TestItem {
            name: "square".into(),
            module_path: "tests.m".into(),
            display_name: Some("squares a number".into()),
            case_label: Some("zero".into()),
            case_index: Some(0),
            ..Default::default()
        };
        assert_eq!(item.display_label(), "squares a number[zero]");
    }

    #[test]
    fn test_item_case_label_omitted_when_none() {
        let item = TestItem {
            name: "plain".into(),
            module_path: "tests.test_math".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&item).expect("serialize");
        assert!(!json.contains("case_label"), "json: {json}");
        assert!(!json.contains("case_index"), "json: {json}");
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
            testing_guard_else_lines: vec![],
            errors: vec![],
        };
        let json = serde_json::to_string(&pf).expect("serialize");
        let back: ParsedFile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(pf, back);
    }
}
