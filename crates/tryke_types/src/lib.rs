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

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExpectedAssertion {
    pub subject: String,
    pub matcher: String,
    pub negated: bool,
    pub args: Vec<String>,
    pub line: u32,
    pub label: Option<String>,
    #[serde(default)]
    pub end_line: u32,
    #[serde(default)]
    pub start_column: Option<u32>,
    #[serde(default)]
    pub end_column: Option<u32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_span: Option<(usize, usize)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_arg_span: Option<(usize, usize)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_arg_value: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Assertion {
    pub expression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// Flat wire format produced by the Python worker's ``run_test`` function.
///
/// This is the JSON shape that comes over the JSON-RPC boundary from the
/// Python subprocess (and from the Pyodide playground runner). Callers
/// convert it into a [`TestResult`] via [`convert_wire_result`].
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunTestResultWire {
    Passed {
        duration_ms: u64,
        stdout: String,
        stderr: String,
    },
    Failed {
        duration_ms: u64,
        message: String,
        #[serde(default)]
        traceback: Option<String>,
        #[serde(default)]
        assertions: Vec<AssertionWire>,
        #[serde(default)]
        executed_lines: Vec<u32>,
        stdout: String,
        stderr: String,
    },
    Skipped {
        duration_ms: u64,
        #[serde(default)]
        reason: Option<String>,
        stdout: String,
        stderr: String,
    },
    #[serde(rename = "xfailed")]
    XFailed {
        duration_ms: u64,
        #[serde(default)]
        reason: Option<String>,
        stdout: String,
        stderr: String,
    },
    #[serde(rename = "xpassed")]
    XPassed {
        duration_ms: u64,
        stdout: String,
        stderr: String,
    },
    Todo {
        duration_ms: u64,
        #[serde(default)]
        description: Option<String>,
        stdout: String,
        stderr: String,
    },
}

/// A single assertion result as serialized by the Python worker.
#[derive(Debug, serde::Deserialize)]
pub struct AssertionWire {
    pub expression: String,
    pub expected: String,
    pub received: String,
    pub line: u32,
    #[serde(default)]
    pub column: Option<u32>,
    #[serde(default)]
    pub file: Option<String>,
}

/// Convert a [`RunTestResultWire`] (flat Python worker format) into a
/// [`TestResult`] (the structured format reporters consume).
///
/// Enriches every assertion in a failed outcome with the matching
/// [`ExpectedAssertion`] discovered statically, so the resulting
/// [`Assertion`] carries the rich `span_offset` / `span_length` /
/// `expected_arg_span` data reporters use for inline diagnostics.
/// Used by both the native worker path and the WASM/playground path.
#[must_use]
pub fn convert_wire_result(test: TestItem, wire: RunTestResultWire) -> TestResult {
    match wire {
        RunTestResultWire::Passed {
            duration_ms,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::Failed {
            duration_ms,
            message,
            traceback,
            assertions,
            executed_lines,
            stdout,
            stderr,
        } => {
            let executed_lines = map_executed_lines(executed_lines, &test.expected_assertions);
            let assertions = assertions
                .into_iter()
                .map(|wire| {
                    let expected_assertion =
                        select_expected_assertion(&test.expected_assertions, &wire);
                    convert_assertion(wire, expected_assertion)
                })
                .collect();
            TestResult {
                test,
                outcome: TestOutcome::Failed {
                    message,
                    traceback,
                    assertions,
                    executed_lines,
                },
                duration: Duration::from_millis(duration_ms),
                stdout,
                stderr,
            }
        }
        RunTestResultWire::Skipped {
            duration_ms,
            reason,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Skipped { reason },
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::XFailed {
            duration_ms,
            reason,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::XFailed { reason },
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::XPassed {
            duration_ms,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::XPassed,
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::Todo {
            duration_ms,
            description,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Todo { description },
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
    }
}

/// Convert a raw [`AssertionWire`] into an [`Assertion`], enriching span /
/// arg-span / line / path data from the optionally-supplied
/// [`ExpectedAssertion`] (statically discovered ahead of time). When no
/// match is provided, the assertion falls back to highlighting the whole
/// expression instead of attempting to re-parse Python syntax from the
/// worker payload.
#[must_use]
pub fn convert_assertion(
    wire: AssertionWire,
    expected_assertion: Option<&ExpectedAssertion>,
) -> Assertion {
    let expression = expected_assertion
        .and_then(|ea| (!ea.expression.is_empty()).then(|| ea.expression.clone()))
        .unwrap_or(wire.expression);
    let (span_offset, span_length) = expected_assertion
        .and_then(|ea| ea.subject_span)
        .unwrap_or((0, expression.len().max(1)));
    let expected_arg_span = expected_assertion.and_then(|ea| ea.expected_arg_span);
    let line = expected_assertion.map_or(wire.line as usize, |ea| ea.line as usize);
    // Make absolute paths relative to cwd so diagnostics show short paths.
    // In WASM `current_dir()` returns `Err` and the unchanged path is used.
    let file = wire.file.map(|f| {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| {
                Path::new(&f)
                    .strip_prefix(&cwd)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or(f)
    });
    Assertion {
        expression,
        file,
        line,
        span_offset,
        span_length,
        expected: wire.expected,
        received: wire.received,
        expected_arg_span,
    }
}

fn expected_end_line(ea: &ExpectedAssertion) -> u32 {
    ea.end_line.max(ea.line)
}

fn expected_contains_line(ea: &ExpectedAssertion, line: u32) -> bool {
    ea.line <= line && line <= expected_end_line(ea)
}

fn expected_contains_position(ea: &ExpectedAssertion, line: u32, column: Option<u32>) -> bool {
    if !expected_contains_line(ea, line) {
        return false;
    }
    let Some(column) = column else {
        return true;
    };
    if line == ea.line
        && let Some(start_column) = ea.start_column
        && column < start_column
    {
        return false;
    }
    if line == expected_end_line(ea)
        && let Some(end_column) = ea.end_column
        && column > end_column
    {
        return false;
    }
    true
}

fn expected_rank(ea: &ExpectedAssertion) -> (u32, usize, u32) {
    (
        expected_end_line(ea) - ea.line,
        ea.expression.len(),
        ea.start_column.unwrap_or(u32::MAX),
    )
}

fn expected_arg_value(ea: &ExpectedAssertion) -> Option<&str> {
    if let Some(value) = ea.expected_arg_value.as_deref() {
        return Some(value.trim());
    }
    let arg = ea.args.first()?.trim();
    if let Some((name, value)) = split_keyword_arg_value(arg) {
        debug_assert!(!name.is_empty());
        Some(value.trim())
    } else {
        Some(arg)
    }
}

fn split_keyword_arg_value(arg: &str) -> Option<(&str, &str)> {
    let (name, value) = arg.split_once('=')?;
    let name = name.trim();
    let value = value.trim();
    if name.is_empty() || value.starts_with('=') || !is_ascii_python_identifier(name) {
        return None;
    }
    Some((name, value))
}

fn is_ascii_python_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn expected_arg_matches_wire(ea: &ExpectedAssertion, wire: &AssertionWire) -> bool {
    expected_arg_value(ea).is_some_and(|arg| arg == wire.expected.trim())
}

fn select_expected_assertion<'a>(
    expected_assertions: &'a [ExpectedAssertion],
    wire: &AssertionWire,
) -> Option<&'a ExpectedAssertion> {
    let line_matches = expected_assertions
        .iter()
        .filter(|ea| expected_contains_line(ea, wire.line))
        .collect::<Vec<_>>();
    if wire.column.is_some() {
        return line_matches
            .iter()
            .copied()
            .filter(|ea| expected_contains_position(ea, wire.line, wire.column))
            .min_by_key(|ea| expected_rank(ea))
            .or_else(|| {
                line_matches
                    .iter()
                    .copied()
                    .min_by_key(|ea| expected_rank(ea))
            });
    }

    let expected_matches = line_matches
        .iter()
        .copied()
        .filter(|ea| expected_arg_matches_wire(ea, wire))
        .collect::<Vec<_>>();
    if let [ea] = expected_matches.as_slice() {
        return Some(*ea);
    }

    let expression_matches = line_matches
        .iter()
        .copied()
        .filter(|ea| !ea.expression.is_empty() && ea.expression == wire.expression)
        .collect::<Vec<_>>();
    if let [ea] = expression_matches.as_slice() {
        return Some(*ea);
    }

    if let [ea] = line_matches.as_slice() {
        Some(*ea)
    } else {
        None
    }
}

fn map_executed_lines(lines: Vec<u32>, expected_assertions: &[ExpectedAssertion]) -> Vec<u32> {
    lines
        .into_iter()
        .map(|line| {
            let mut matches = expected_assertions
                .iter()
                .filter(|ea| expected_contains_line(ea, line));
            match (matches.next(), matches.next()) {
                (Some(ea), None) => ea.line,
                _ => line,
            }
        })
        .collect()
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

impl RunSummary {
    /// Aggregate outcome counts and total duration from a flat slice of
    /// [`TestResult`]s. Leaves discovery/start-time/changed-selection
    /// fields at their defaults — those carry information the caller
    /// (CLI execution loop, server, playground) must supply itself.
    #[must_use]
    pub fn from_results(results: &[TestResult]) -> Self {
        let mut summary = Self {
            passed: 0,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::ZERO,
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        };
        for r in results {
            summary.duration += r.duration;
            match &r.outcome {
                TestOutcome::Passed => summary.passed += 1,
                TestOutcome::Failed { .. } | TestOutcome::XPassed => summary.failed += 1,
                TestOutcome::Skipped { .. } => summary.skipped += 1,
                TestOutcome::Error { .. } => summary.errors += 1,
                TestOutcome::XFailed { .. } => summary.xfailed += 1,
                TestOutcome::Todo { .. } => summary.todo += 1,
            }
        }
        summary.test_duration = Some(summary.duration);
        summary
    }
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

    Scheduler,
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

/// Everything derivable from a single parse of a Python source file:
/// the `ParsedFile` (tests, hooks, guard-else lines, errors), the
/// candidate import paths this file references, and the dynamic-import
/// flag. Produced in one AST walk so callers never parse the file
/// twice. `import_candidates` holds first-wins alternatives that the
/// discoverer resolves against the project's enumerated file set,
/// avoiding per-import `stat()` syscalls.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredFile {
    pub parsed: ParsedFile,
    pub import_candidates: Vec<Vec<PathBuf>>,
    pub dynamic_imports: bool,
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

    #[test]
    fn convert_wire_result_passed() {
        let test = TestItem {
            name: "test_add".into(),
            module_path: "tests.test_math".into(),
            ..Default::default()
        };
        let wire = RunTestResultWire::Passed {
            duration_ms: 10,
            stdout: String::new(),
            stderr: String::new(),
        };
        let result = convert_wire_result(test, wire);
        assert!(matches!(result.outcome, TestOutcome::Passed));
        assert_eq!(result.duration, Duration::from_millis(10));
    }

    #[test]
    fn convert_assertion_without_discovery_highlights_whole_expression() {
        let wire = AssertionWire {
            expression: "expect(x).to_equal(2)".into(),
            expected: "2".into(),
            received: "3".into(),
            line: 10,
            column: None,
            file: Some("tests/test_math.py".into()),
        };
        let a = convert_assertion(wire, None);
        assert_eq!(a.span_offset, 0);
        assert_eq!(a.span_length, "expect(x).to_equal(2)".len());
        assert_eq!(a.expected_arg_span, None);
    }

    #[test]
    fn convert_assertion_maps_wire_fields() {
        let wire = AssertionWire {
            expression: "expect(x).to_equal(2)".into(),
            expected: "2".into(),
            received: "3".into(),
            line: 10,
            column: None,
            file: Some("tests/test_math.py".into()),
        };
        let expected = ExpectedAssertion {
            subject: "x".into(),
            matcher: "to_equal".into(),
            args: vec!["2".into()],
            line: 10,
            subject_span: Some((7, 1)),
            expected_arg_span: Some((19, 1)),
            ..Default::default()
        };
        let a = convert_assertion(wire, Some(&expected));
        assert_eq!(a.expression, "expect(x).to_equal(2)");
        assert_eq!(a.expected, "2");
        assert_eq!(a.received, "3");
        assert_eq!(a.line, 10);
        assert_eq!(a.file.as_deref(), Some("tests/test_math.py"));
        assert_eq!(a.span_offset, 7);
        assert_eq!(a.span_length, 1);
        assert_eq!(a.expected_arg_span, Some((19, 1)));
    }

    #[test]
    fn expected_arg_value_only_splits_keyword_arguments() {
        let positional = ExpectedAssertion {
            args: vec!["x == y".into()],
            ..Default::default()
        };
        assert_eq!(expected_arg_value(&positional), Some("x == y"));

        let keyword = ExpectedAssertion {
            args: vec!["other = 1".into()],
            ..Default::default()
        };
        assert_eq!(expected_arg_value(&keyword), Some("1"));

        let discovered_value = ExpectedAssertion {
            args: vec!["other=x == y".into()],
            expected_arg_value: Some("x == y".into()),
            ..Default::default()
        };
        assert_eq!(expected_arg_value(&discovered_value), Some("x == y"));
    }
}
