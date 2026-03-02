use std::{
    env, fs,
    path::{Path, PathBuf},
};

use ignore::WalkBuilder;
use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::parse_module;
use ruff_source_file::LineIndex;
use ruff_text_size::Ranged;
use tryke_types::{ExpectedAssertion, TestItem};

fn find_project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join("pyproject.toml").exists())
        .map(Path::to_path_buf)
}

fn collect_python_files(root: &Path) -> Vec<PathBuf> {
    WalkBuilder::new(root)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|ext| ext == "py"))
        .collect()
}

fn path_to_module(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .with_extension("")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(".")
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

fn parse_tests_from_file(root: &Path, file: &Path) -> Vec<TestItem> {
    let Ok(source) = fs::read_to_string(file) else {
        return vec![];
    };
    let Ok(parsed) = parse_module(&source) else {
        return vec![];
    };
    let line_index = LineIndex::from_source_text(&source);
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
                        &source,
                        &line_index,
                    ),
                })
            } else {
                None
            }
        })
        .collect()
}

#[must_use]
pub fn discover() -> Vec<TestItem> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = find_project_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut files = collect_python_files(&root);
    files.sort();
    files
        .iter()
        .flat_map(|f| parse_tests_from_file(&root, f))
        .collect()
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
}
