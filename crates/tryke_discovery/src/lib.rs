use std::{
    env, fs,
    path::{Path, PathBuf},
};

use log::trace;
use rayon::prelude::*;

pub(crate) mod db;
mod discoverer;
pub(crate) mod import_graph;
pub use discoverer::Discoverer;

use ignore::WalkBuilder;
use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::parse_module;
use ruff_source_file::LineIndex;
use ruff_text_size::Ranged;
use tryke_types::{ExpectedAssertion, TestItem};

pub(crate) fn find_project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join("pyproject.toml").exists())
        .map(Path::to_path_buf)
}

pub(crate) fn collect_python_files(root: &Path) -> Vec<PathBuf> {
    WalkBuilder::new(root)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|ext| ext == "py"))
        .collect()
}

pub(crate) fn path_to_module(root: &Path, file: &Path) -> String {
    tryke_types::path_to_module(root, file).unwrap_or_default()
}

/// Resolve `module_name` (e.g. "foo.bar") as a local file under `root`.
/// Tries `root/foo/bar.py` then `root/foo/bar/__init__.py`.
fn resolve_absolute_import(root: &Path, module_name: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in module_name.split('.') {
        path = path.join(part);
    }
    let py = path.with_extension("py");
    if py.starts_with(root) && py.exists() {
        return Some(py);
    }
    let init = path.join("__init__.py");
    if init.starts_with(root) && init.exists() {
        return Some(init);
    }
    None
}

/// Resolve a module path from `base` directory.
/// If `module_name` is empty, tries `base/__init__.py`.
fn resolve_relative_import_path(root: &Path, base: &Path, module_name: &str) -> Option<PathBuf> {
    if module_name.is_empty() {
        let init = base.join("__init__.py");
        if init.starts_with(root) && init.exists() {
            return Some(init);
        }
        return None;
    }
    let mut path = base.to_path_buf();
    for part in module_name.split('.') {
        path = path.join(part);
    }
    let py = path.with_extension("py");
    if py.starts_with(root) && py.exists() {
        return Some(py);
    }
    let init = path.join("__init__.py");
    if init.starts_with(root) && init.exists() {
        return Some(init);
    }
    None
}

/// Extract local file imports from a pre-parsed Python module body.
/// Returns absolute paths of project-local files that this file imports.
pub(crate) fn extract_local_imports(root: &Path, file: &Path, body: &[Stmt]) -> Vec<PathBuf> {
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut result: Vec<PathBuf> = Vec::new();

    let mut add = |p: PathBuf| {
        if seen.insert(p.clone()) {
            result.push(p);
        }
    };

    for stmt in body {
        match stmt {
            Stmt::Import(import_stmt) => {
                for alias in &import_stmt.names {
                    let module_name = alias.name.id.as_str();
                    if let Some(path) = resolve_absolute_import(root, module_name) {
                        add(path);
                    }
                }
            }
            Stmt::ImportFrom(from_stmt) => {
                let level = from_stmt.level;
                if level == 0 {
                    // Absolute: from foo.bar import x
                    if let Some(module) = &from_stmt.module
                        && let Some(path) = resolve_absolute_import(root, module.id.as_str())
                    {
                        add(path);
                    }
                } else {
                    // Relative: walk up level-1 directories from file's parent
                    let mut base = file.parent().map(Path::to_path_buf);
                    for _ in 0..level.saturating_sub(1) {
                        base = base.and_then(|b| b.parent().map(Path::to_path_buf));
                    }
                    if let Some(base) = base {
                        if let Some(module) = &from_stmt.module {
                            // from .utils import x → resolve "utils" from base
                            if let Some(path) =
                                resolve_relative_import_path(root, &base, module.id.as_str())
                            {
                                add(path);
                            }
                        } else {
                            // from . import x, y → try each name as a submodule
                            for alias in &from_stmt.names {
                                let name = alias.name.id.as_str();
                                if let Some(path) = resolve_relative_import_path(root, &base, name)
                                {
                                    add(path);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    result
}

fn is_locally_defined(name: &str, body: &[Stmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        Stmt::FunctionDef(f) => f.name.id.as_str() == name,
        Stmt::ClassDef(c) => c.name.id.as_str() == name,
        Stmt::Assign(a) => a
            .targets
            .iter()
            .any(|t| matches!(t, Expr::Name(n) if n.id.as_str() == name)),
        Stmt::AnnAssign(a) => matches!(&*a.target, Expr::Name(n) if n.id.as_str() == name),
        _ => false,
    })
}

fn is_tryke_test_decorator(expr: &Expr, body: &[Stmt]) -> bool {
    match expr {
        Expr::Attribute(a) => {
            a.attr.id.as_str() == "test"
                && matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "tryke")
        }
        Expr::Name(n) => n.id.as_str() == "test" && !is_locally_defined("test", body),
        Expr::Call(c) => is_tryke_test_decorator(&c.func, body),
        _ => false,
    }
}

fn extract_decorator_name(expr: &Expr) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    for kw in &call.arguments.keywords {
        if kw.arg.as_ref().is_some_and(|k| k.id.as_str() == "name")
            && let Expr::StringLiteral(s) = &kw.value
        {
            return Some(s.value.to_str().to_owned());
        }
    }
    if let Some(first) = call.arguments.args.first()
        && let Expr::StringLiteral(s) = first
    {
        return Some(s.value.to_str().to_owned());
    }
    None
}

fn extract_docstring(body: &[Stmt]) -> Option<String> {
    if let Some(Stmt::Expr(s)) = body.first()
        && let Expr::StringLiteral(lit) = &*s.value
    {
        let text = lit.value.to_str();
        return Some(text.lines().next().unwrap_or("").trim().to_owned());
    }
    None
}

fn src_text(source: &str, range: ruff_text_size::TextRange) -> String {
    let start: usize = range.start().into();
    let end: usize = range.end().into();
    source[start..end].to_owned()
}

fn extract_expect_call_info(
    call: &ruff_python_ast::ExprCall,
    source: &str,
) -> Option<(String, Option<String>)> {
    let is_expect = match call.func.as_ref() {
        Expr::Name(n) => n.id.as_str() == "expect",
        Expr::Attribute(a) => a.attr.id.as_str() == "expect",
        _ => return None,
    };
    let nargs = call.arguments.args.len();
    if !is_expect || nargs == 0 || nargs > 2 {
        return None;
    }
    let subject = src_text(source, call.arguments.args[0].range());
    let label = call
        .arguments
        .keywords
        .iter()
        .find_map(|kw| {
            if kw.arg.as_ref().is_some_and(|k| k.id.as_str() == "name")
                && let Expr::StringLiteral(s) = &kw.value
            {
                return Some(s.value.to_str().to_owned());
            }
            None
        })
        .or_else(|| {
            if let Some(Expr::StringLiteral(s)) = call.arguments.args.get(1) {
                Some(s.value.to_str().to_owned())
            } else {
                None
            }
        });
    Some((subject, label))
}

fn try_extract_assertion(
    call: &ruff_python_ast::ExprCall,
    source: &str,
    line_index: &LineIndex,
) -> Option<ExpectedAssertion> {
    // Unwrap `.fatal()`: expect(x).to_equal(y).fatal() wraps the assertion call
    if let Expr::Attribute(attr) = call.func.as_ref()
        && attr.attr.id.as_str() == "fatal"
        && let Expr::Call(inner_call) = attr.value.as_ref()
    {
        return try_extract_assertion(inner_call, source, line_index);
    }

    let Expr::Attribute(outer_attr) = call.func.as_ref() else {
        return None;
    };
    let matcher = outer_attr.attr.id.as_str().to_owned();
    let (subject, negated, label) = match outer_attr.value.as_ref() {
        Expr::Call(inner_call) => {
            let (subject, label) = extract_expect_call_info(inner_call, source)?;
            (subject, false, label)
        }
        Expr::Attribute(inner_attr) if inner_attr.attr.id.as_str() == "not_" => {
            let Expr::Call(inner_call) = inner_attr.value.as_ref() else {
                return None;
            };
            let (subject, label) = extract_expect_call_info(inner_call, source)?;
            (subject, true, label)
        }
        _ => return None,
    };
    let args = call
        .arguments
        .args
        .iter()
        .map(|a| src_text(source, a.range()))
        .collect();
    let line = u32::try_from(line_index.line_index(call.range.start()).get()).unwrap_or(0);
    Some(ExpectedAssertion {
        subject,
        matcher,
        negated,
        args,
        line,
        label,
    })
}

fn collect_assertions_from_expr(
    expr: &Expr,
    source: &str,
    line_index: &LineIndex,
    out: &mut Vec<ExpectedAssertion>,
) {
    if let Expr::Call(call) = expr {
        if let Some(a) = try_extract_assertion(call, source, line_index) {
            out.push(a);
            for arg in &call.arguments.args {
                collect_assertions_from_expr(arg, source, line_index, out);
            }
            return;
        }
        collect_assertions_from_expr(&call.func, source, line_index, out);
        for arg in &call.arguments.args {
            collect_assertions_from_expr(arg, source, line_index, out);
        }
    }
}

fn collect_assertions_from_stmt(
    stmt: &Stmt,
    source: &str,
    line_index: &LineIndex,
    out: &mut Vec<ExpectedAssertion>,
) {
    match stmt {
        Stmt::Expr(s) => collect_assertions_from_expr(&s.value, source, line_index, out),
        Stmt::Return(s) => {
            if let Some(v) = &s.value {
                collect_assertions_from_expr(v, source, line_index, out);
            }
        }
        Stmt::If(s) => {
            collect_assertions_from_expr(&s.test, source, line_index, out);
            for inner in &s.body {
                collect_assertions_from_stmt(inner, source, line_index, out);
            }
            for clause in &s.elif_else_clauses {
                if let Some(test) = &clause.test {
                    collect_assertions_from_expr(test, source, line_index, out);
                }
                for inner in &clause.body {
                    collect_assertions_from_stmt(inner, source, line_index, out);
                }
            }
        }
        Stmt::For(s) => {
            for inner in s.body.iter().chain(s.orelse.iter()) {
                collect_assertions_from_stmt(inner, source, line_index, out);
            }
        }
        Stmt::While(s) => {
            for inner in s.body.iter().chain(s.orelse.iter()) {
                collect_assertions_from_stmt(inner, source, line_index, out);
            }
        }
        Stmt::With(s) => {
            for inner in &s.body {
                collect_assertions_from_stmt(inner, source, line_index, out);
            }
        }
        Stmt::Try(s) => {
            for inner in s
                .body
                .iter()
                .chain(s.orelse.iter())
                .chain(s.finalbody.iter())
            {
                collect_assertions_from_stmt(inner, source, line_index, out);
            }
        }
        _ => {}
    }
}

fn extract_expected_assertions(
    body: &[Stmt],
    source: &str,
    line_index: &LineIndex,
) -> Vec<ExpectedAssertion> {
    let mut out = Vec::new();
    for stmt in body {
        collect_assertions_from_stmt(stmt, source, line_index, &mut out);
    }
    out
}

pub(crate) fn parse_tests_from_source(root: &Path, file: &Path, source: &str) -> Vec<TestItem> {
    trace!(
        "parsing {}",
        file.strip_prefix(root).unwrap_or(file).display()
    );
    let Ok(parsed) = parse_module(source) else {
        trace!("parse error in {}", file.display());
        return vec![];
    };
    let line_index = LineIndex::from_source_text(source);
    let module = parsed.syntax();
    let body = &module.body;
    body.iter()
        .filter_map(|stmt| {
            if let Stmt::FunctionDef(func) = stmt
                && func
                    .decorator_list
                    .iter()
                    .any(|d| is_tryke_test_decorator(&d.expression, body))
            {
                let display_name = func
                    .decorator_list
                    .iter()
                    .find(|d| is_tryke_test_decorator(&d.expression, body))
                    .and_then(|d| extract_decorator_name(&d.expression))
                    .or_else(|| extract_docstring(&func.body));
                Some(TestItem {
                    name: func.name.id.as_str().to_owned(),
                    module_path: path_to_module(root, file),
                    file_path: Some(file.strip_prefix(root).unwrap_or(file).to_path_buf()),
                    line_number: u32::try_from(line_index.line_index(func.range.start()).get())
                        .ok(),
                    display_name,
                    expected_assertions: extract_expected_assertions(
                        &func.body,
                        source,
                        &line_index,
                    ),
                })
            } else {
                None
            }
        })
        .collect()
}

fn parse_tests_from_file(root: &Path, file: &Path) -> Vec<TestItem> {
    let Ok(source) = fs::read_to_string(file) else {
        return vec![];
    };
    parse_tests_from_source(root, file, &source)
}

#[must_use]
pub fn discover_from(start: &Path) -> Vec<TestItem> {
    let root = find_project_root(start).unwrap_or_else(|| start.to_path_buf());
    let mut files = collect_python_files(&root);
    files.sort();
    files
        .par_iter()
        .flat_map_iter(|f| parse_tests_from_file(&root, f))
        .collect()
}

/// # Errors
/// Returns an error if the current directory cannot be determined.
pub fn discover() -> std::io::Result<Vec<TestItem>> {
    let cwd = env::current_dir()?;
    Ok(discover_from(&cwd))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn make_tree(files: &[&str]) -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        for rel in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create_dir_all");
            }
            fs::write(&path, "").expect("write file");
        }
        dir
    }

    fn write_source(source: &str) -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("test.py");
        fs::write(&file, source).expect("write source");
        (dir, file)
    }

    #[test]
    fn finds_project_root_from_child_dir() {
        let dir = make_tree(&["src/foo.py"]);
        let child = dir.path().join("src");
        assert_eq!(find_project_root(&child), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn returns_none_when_no_pyproject() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(find_project_root(dir.path()), None);
    }

    #[test]
    fn collects_py_files_only() {
        let dir = make_tree(&["a.py", "b.txt", "sub/c.py"]);
        let mut files = collect_python_files(dir.path());
        files.sort();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|p| p.extension().unwrap() == "py"));
    }

    #[test]
    fn respects_ignore_files() {
        let dir = make_tree(&["a.py", "ignored/b.py"]);
        fs::write(dir.path().join(".ignore"), "ignored/\n").expect("write .ignore");
        let files = collect_python_files(dir.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("a.py"));
    }

    #[test]
    fn path_to_module_converts_correctly() {
        let root = Path::new("/proj");
        assert_eq!(
            path_to_module(root, Path::new("/proj/tests/math.py")),
            "tests.math"
        );
        assert_eq!(path_to_module(root, Path::new("/proj/foo.py")), "foo");
    }

    #[test]
    fn extracts_test_decorated_functions() {
        let source = "@test
def test_one():
    pass

@test
def test_two():
    pass

def not_a_test():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 2);
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"test_one"));
        assert!(names.contains(&"test_two"));
    }

    #[test]
    fn skips_non_test_decorators() {
        let source = "@pytest.mark.skip
def test_skipped():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn captures_correct_line_number() {
        // @test is on line 3 (1-indexed), func range starts at the decorator
        let source = "

@test
def test_fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].line_number, Some(3));
    }

    #[test]
    fn returns_empty_for_parse_error() {
        let source = "this is not valid python @@@";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn returns_empty_for_unreadable_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nonexistent = dir.path().join("nonexistent.py");
        let items = parse_tests_from_file(dir.path(), &nonexistent);
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn skips_locally_defined_test_function() {
        let source = "def test(fn):
    return fn
@test
def my_func():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn skips_assigned_test_name() {
        let source = "test = lambda fn: fn
@test
def my_func():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn recognizes_qualified_tryke_test() {
        let source = "import tryke
@tryke.test
def my_func():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "my_func");
    }

    #[test]
    fn qualified_form_overrides_local_definition() {
        let source = "def test(fn):
    return fn
import tryke
@tryke.test
def my_func():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "my_func");
    }

    #[test]
    fn extracts_simple_assertion() {
        let source = "@test
def test_fn():
    expect(add(1, 1)).to_equal(2)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 1);
        let assertions = &items[0].expected_assertions;
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].subject, "add(1, 1)");
        assert_eq!(assertions[0].matcher, "to_equal");
        assert!(!assertions[0].negated);
        assert_eq!(assertions[0].args, vec!["2"]);
    }

    #[test]
    fn extracts_negated_assertion() {
        let source = "@test
def test_fn():
    expect(x).not_.to_be_none()
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].expected_assertions.len(), 1);
        let a = &items[0].expected_assertions[0];
        assert_eq!(a.subject, "x");
        assert_eq!(a.matcher, "to_be_none");
        assert!(a.negated);
        assert!(a.args.is_empty());
    }

    #[test]
    fn extracts_multiple_assertions() {
        let source = "@test
def test_fn():
    expect(a).to_equal(1)
    expect(b).to_equal(2)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].expected_assertions.len(), 2);
    }

    #[test]
    fn no_assertions_when_none_present() {
        let source = "@test
def test_fn():
    result = add(1, 1)
    assert result == 2
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].expected_assertions.len(), 0);
    }

    #[test]
    fn extracts_assertion_with_line_number() {
        let source = "@test
def test_fn():
    pass
    expect(x).to_equal(1)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].expected_assertions.len(), 1);
        assert_eq!(items[0].expected_assertions[0].line, 4);
    }

    #[test]
    fn recognizes_call_form_decorator() {
        let source = "@test()
def test_fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "test_fn");
    }

    #[test]
    fn display_name_from_positional_string() {
        let source = "@test(\"addition works\")
def test_fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].display_name.as_deref(), Some("addition works"));
    }

    #[test]
    fn display_name_from_name_kwarg() {
        let source = "@test(name=\"my label\")
def test_fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].display_name.as_deref(), Some("my label"));
    }

    #[test]
    fn display_name_kwarg_beats_positional() {
        let source = "@test(\"pos\", name=\"kwarg\")
def test_fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].display_name.as_deref(), Some("kwarg"));
    }

    #[test]
    fn display_name_from_docstring() {
        let source = "@test
def test_fn():
    \"\"\"docstring name\"\"\"
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].display_name.as_deref(), Some("docstring name"));
    }

    #[test]
    fn decorator_name_beats_docstring() {
        let source = "@test(name=\"explicit\")
def test_fn():
    \"\"\"docstring\"\"\"
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].display_name.as_deref(), Some("explicit"));
    }

    #[test]
    fn bare_test_no_display_name() {
        let source = "@test
def test_fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].display_name, None);
    }

    #[test]
    fn expect_label_from_name_kwarg() {
        let source = "@test
def test_fn():
    expect(x, name=\"my label\").to_equal(1)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        let a = &items[0].expected_assertions[0];
        assert_eq!(a.label.as_deref(), Some("my label"));
        assert_eq!(a.subject, "x");
    }

    #[test]
    fn expect_label_from_positional_string() {
        let source = "@test
def test_fn():
    expect(x, \"my label\").to_equal(1)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        let a = &items[0].expected_assertions[0];
        assert_eq!(a.label.as_deref(), Some("my label"));
        assert_eq!(a.subject, "x");
    }

    #[test]
    fn expect_name_kwarg_beats_positional_label() {
        let source = "@test
def test_fn():
    expect(x, \"pos\", name=\"kw\").to_equal(1)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].expected_assertions[0].label.as_deref(), Some("kw"));
    }

    #[test]
    fn expect_no_label_by_default() {
        let source = "@test
def test_fn():
    expect(x).to_equal(1)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].expected_assertions[0].label, None);
    }

    #[test]
    fn discover_from_finds_tests_in_given_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        fs::write(
            dir.path().join("test_example.py"),
            "@test\ndef test_hello():\n    pass\n",
        )
        .expect("write test file");
        let items = discover_from(dir.path());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "test_hello");
    }

    #[test]
    fn tryke_test_call_form_qualified() {
        let source = "import tryke
@tryke.test(name=\"foo\")
def my_func():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].display_name.as_deref(), Some("foo"));
    }

    #[test]
    fn extract_local_imports_absolute() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("utils.py"), "").expect("write");
        let source = "import utils\n";
        let parsed = parse_module(source).expect("parse");
        let file = root.join("test_foo.py");
        let imports = extract_local_imports(root, &file, &parsed.syntax().body);
        assert_eq!(imports, vec![root.join("utils.py")]);
    }

    #[test]
    fn extract_local_imports_from_absolute() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("utils.py"), "").expect("write");
        let source = "from utils import helper\n";
        let parsed = parse_module(source).expect("parse");
        let file = root.join("test_foo.py");
        let imports = extract_local_imports(root, &file, &parsed.syntax().body);
        assert_eq!(imports, vec![root.join("utils.py")]);
    }

    #[test]
    fn extract_local_imports_ignores_nonlocal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // stdlib / third-party (doesn't exist under root)
        let source = "import os\nimport pytest\n";
        let parsed = parse_module(source).expect("parse");
        let file = root.join("test_foo.py");
        let imports = extract_local_imports(root, &file, &parsed.syntax().body);
        assert!(imports.is_empty());
    }

    #[test]
    fn extract_local_imports_relative() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sub = root.join("pkg");
        fs::create_dir_all(&sub).expect("mkdir");
        fs::write(sub.join("utils.py"), "").expect("write");
        let source = "from .utils import helper\n";
        let parsed = parse_module(source).expect("parse");
        let file = sub.join("test_foo.py");
        let imports = extract_local_imports(root, &file, &parsed.syntax().body);
        assert_eq!(imports, vec![sub.join("utils.py")]);
    }

    #[test]
    fn extract_local_imports_relative_parent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sub = root.join("pkg").join("sub");
        fs::create_dir_all(&sub).expect("mkdir");
        fs::write(root.join("pkg").join("utils.py"), "").expect("write");
        let source = "from ..utils import helper\n";
        let parsed = parse_module(source).expect("parse");
        let file = sub.join("test_foo.py");
        let imports = extract_local_imports(root, &file, &parsed.syntax().body);
        assert_eq!(imports, vec![root.join("pkg").join("utils.py")]);
    }

    #[test]
    fn extract_local_imports_from_dot_import_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sub = root.join("pkg");
        fs::create_dir_all(&sub).expect("mkdir");
        fs::write(sub.join("helpers.py"), "").expect("write");
        let source = "from . import helpers\n";
        let parsed = parse_module(source).expect("parse");
        let file = sub.join("test_foo.py");
        let imports = extract_local_imports(root, &file, &parsed.syntax().body);
        assert_eq!(imports, vec![sub.join("helpers.py")]);
    }

    #[test]
    fn extract_local_imports_resolves_package_init() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sub = root.join("mypkg");
        fs::create_dir_all(&sub).expect("mkdir");
        fs::write(sub.join("__init__.py"), "").expect("write");
        let source = "import mypkg\n";
        let parsed = parse_module(source).expect("parse");
        let file = root.join("test_foo.py");
        let imports = extract_local_imports(root, &file, &parsed.syntax().body);
        assert_eq!(imports, vec![sub.join("__init__.py")]);
    }

    #[test]
    fn extract_local_imports_deduplicates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("utils.py"), "").expect("write");
        let source = "import utils\nimport utils\n";
        let parsed = parse_module(source).expect("parse");
        let file = root.join("test_foo.py");
        let imports = extract_local_imports(root, &file, &parsed.syntax().body);
        assert_eq!(imports.len(), 1);
    }

    #[test]
    fn extracts_assertion_with_fatal() {
        let source = "@test
def test_fn():
    expect(x).to_equal(1).fatal()
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].expected_assertions.len(), 1);
        let a = &items[0].expected_assertions[0];
        assert_eq!(a.subject, "x");
        assert_eq!(a.matcher, "to_equal");
        assert!(!a.negated);
        assert_eq!(a.args, vec!["1"]);
    }

    #[test]
    fn extracts_negated_assertion_with_fatal() {
        let source = "@test
def test_fn():
    expect(x).not_.to_be_none().fatal()
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &file);
        assert_eq!(items[0].expected_assertions.len(), 1);
        let a = &items[0].expected_assertions[0];
        assert_eq!(a.subject, "x");
        assert_eq!(a.matcher, "to_be_none");
        assert!(a.negated);
    }
}
