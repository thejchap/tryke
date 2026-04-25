use std::{
    env, fs,
    path::{Path, PathBuf},
};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use log::trace;
use rayon::prelude::*;

pub(crate) mod cache;
pub(crate) mod db;
mod discoverer;
pub(crate) mod import_graph;
pub use discoverer::Discoverer;

use ignore::WalkBuilder;
use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::parse_module;
use ruff_source_file::LineIndex;
use ruff_text_size::Ranged;
use tryke_types::{ExpectedAssertion, FixturePer, HookItem, ParsedFile, TestItem};

pub(crate) fn find_project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join("pyproject.toml").exists())
        .map(Path::to_path_buf)
}

#[must_use]
pub fn configured_excludes(start: &Path, cli_excludes: &[String]) -> Vec<String> {
    if !cli_excludes.is_empty() {
        return cli_excludes.to_vec();
    }
    tryke_config::load_effective_config(start).discovery.exclude
}

fn build_excludes(root: &Path, excludes: &[String]) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    for exclude in excludes {
        let _ = builder.add_line(None, exclude);
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

pub(crate) fn collect_python_files(root: &Path, excludes: &[String]) -> Vec<PathBuf> {
    let exclude_matcher = build_excludes(root, excludes);
    WalkBuilder::new(root)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|ext| ext == "py"))
        .filter(|p| {
            !exclude_matcher
                .matched_path_or_any_parents(p, false)
                .is_ignore()
        })
        .collect()
}

/// Like `collect_python_files`, but walks only the supplied `walk_roots`
/// instead of the whole project. Used by the path-restricted discovery
/// fast path (`Discoverer::rediscover_restricted`) so a `tryke test
/// path/to/foo.py` invocation doesn't pay an O(project-files) walk.
///
/// `walk_roots` may contain absolute paths to either directories or
/// individual `.py` files. The exclude matcher is anchored at
/// `project_root` so `pyproject.toml` exclude patterns evaluate against
/// project-relative paths exactly as in the full walk.
pub(crate) fn collect_python_files_restricted(
    project_root: &Path,
    walk_roots: &[PathBuf],
    excludes: &[String],
) -> Vec<PathBuf> {
    let exclude_matcher = build_excludes(project_root, excludes);
    let is_excluded = |p: &Path| -> bool {
        exclude_matcher
            .matched_path_or_any_parents(p, false)
            .is_ignore()
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for walk_root in walk_roots {
        // `WalkBuilder::new(p)` works on either a directory or a single
        // file; routing both through it keeps gitignore / .ignore / hidden
        // semantics consistent with `collect_python_files`.
        for entry in WalkBuilder::new(walk_root).build().filter_map(Result::ok) {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.into_path();
            if path.extension().is_none_or(|ext| ext != "py") {
                continue;
            }
            if is_excluded(&path) {
                continue;
            }
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

pub(crate) fn path_to_module(root: &Path, file: &Path) -> String {
    tryke_types::path_to_module(root, file).unwrap_or_default()
}

/// The paths a Python importer would try, in order, for a dotted
/// absolute import `foo.bar` across each configured source root.
/// Under each root, `root/foo/bar.py` comes before
/// `root/foo/bar/__init__.py` (matching Python's import precedence);
/// roots earlier in the slice take precedence over later roots.
/// Candidates that don't start with their root are dropped (they could
/// never resolve to a project-local file anyway).
///
/// No filesystem access — the caller decides which candidate, if any,
/// actually exists. See `resolve_import_candidate_groups`.
fn candidate_absolute_import_paths(src_roots: &[PathBuf], module_name: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(src_roots.len() * 2);
    for root in src_roots {
        let mut path = root.clone();
        for part in module_name.split('.') {
            path = path.join(part);
        }
        let py = path.with_extension("py");
        let init = path.join("__init__.py");
        for candidate in [py, init] {
            if candidate.starts_with(root) {
                out.push(candidate);
            }
        }
    }
    out
}

/// Candidates for a relative import (`from .foo import bar`) walked up
/// `level-1` directories from `file`'s parent. Mirrors the old
/// `resolve_relative_import_path` two-candidate behaviour without any
/// filesystem access.
fn candidate_relative_import_paths(root: &Path, base: &Path, module_name: &str) -> Vec<PathBuf> {
    if module_name.is_empty() {
        let init = base.join("__init__.py");
        return if init.starts_with(root) {
            vec![init]
        } else {
            Vec::new()
        };
    }
    let mut path = base.to_path_buf();
    for part in module_name.split('.') {
        path = path.join(part);
    }
    let py = path.with_extension("py");
    let init = path.join("__init__.py");
    [py, init]
        .into_iter()
        .filter(|p| p.starts_with(root))
        .collect()
}

/// A first-wins group of candidate import paths: the project-local file
/// is whichever candidate in the group exists first, per Python's
/// import semantics (plain `.py` before the package `__init__.py`). The
/// outer caller resolves against a `HashSet` of enumerated project files.
pub(crate) type ImportCandidateGroup = Vec<PathBuf>;

/// Resolve candidate groups against a `HashSet` of project-local files,
/// preserving insertion order and deduplicating across groups. Picks the
/// first existing candidate within each group, matching the old
/// `resolve_absolute_import` / `resolve_relative_import_path` contract.
pub(crate) fn resolve_import_candidate_groups(
    groups: &[ImportCandidateGroup],
    project_files: &std::collections::HashSet<PathBuf>,
) -> Vec<PathBuf> {
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for group in groups {
        for candidate in group {
            if project_files.contains(candidate) {
                if seen.insert(candidate.clone()) {
                    out.push(candidate.clone());
                }
                break;
            }
        }
    }
    out
}

/// Extract candidate import groups from a parsed Python module body.
/// Each group is a first-wins alternatives list; the caller resolves
/// each group against the project file set via
/// `resolve_import_candidate_groups`. No filesystem access.
pub(crate) fn extract_local_import_candidate_groups(
    root: &Path,
    src_roots: &[PathBuf],
    file: &Path,
    body: &[Stmt],
) -> Vec<ImportCandidateGroup> {
    let mut groups: Vec<ImportCandidateGroup> = Vec::new();
    collect_local_import_candidate_groups(root, src_roots, file, body, &mut groups);
    groups
}

fn collect_local_import_candidate_groups(
    root: &Path,
    src_roots: &[PathBuf],
    file: &Path,
    body: &[Stmt],
    groups: &mut Vec<ImportCandidateGroup>,
) {
    for stmt in body {
        match stmt {
            Stmt::Import(import_stmt) => {
                for alias in &import_stmt.names {
                    let module_name = alias.name.id.as_str();
                    let candidates = candidate_absolute_import_paths(src_roots, module_name);
                    if !candidates.is_empty() {
                        groups.push(candidates);
                    }
                }
            }
            Stmt::ImportFrom(from_stmt) => {
                let level = from_stmt.level;
                if level == 0 {
                    // Absolute: from foo.bar import x
                    if let Some(module) = &from_stmt.module {
                        let module_name = module.id.as_str();
                        let candidates = candidate_absolute_import_paths(src_roots, module_name);
                        if !candidates.is_empty() {
                            groups.push(candidates);
                        }
                        for alias in &from_stmt.names {
                            let imported = alias.name.id.as_str();
                            let submodule = format!("{module_name}.{imported}");
                            let candidates = candidate_absolute_import_paths(src_roots, &submodule);
                            if !candidates.is_empty() {
                                groups.push(candidates);
                            }
                        }
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
                            let candidates =
                                candidate_relative_import_paths(root, &base, module.id.as_str());
                            if !candidates.is_empty() {
                                groups.push(candidates);
                            }
                        } else {
                            // from . import x, y → try each name as a submodule
                            for alias in &from_stmt.names {
                                let name = alias.name.id.as_str();
                                let candidates = candidate_relative_import_paths(root, &base, name);
                                if !candidates.is_empty() {
                                    groups.push(candidates);
                                }
                            }
                        }
                    }
                }
            }
            Stmt::Assign(s) => {
                // PEP 810 transitional mechanism: a package `__init__.py` may
                // declare `__lazy_modules__ = ["sub_a", "sub_b"]` to mark its
                // submodules as lazily-importable from the package on Python
                // 3.15+. Treat each entry as a static dependency so that
                // editing the submodule re-runs tests that touch the package.
                if let Some(names) = lazy_modules_targets(&s.targets, &s.value) {
                    push_lazy_module_sibling_groups(root, file, &names, groups);
                }
            }
            Stmt::AnnAssign(s) => {
                // `__lazy_modules__: list[str] = [...]` form.
                if let Some(value) = s.value.as_deref()
                    && let Some(names) = lazy_modules_ann_target(&s.target, value)
                {
                    push_lazy_module_sibling_groups(root, file, &names, groups);
                }
            }
            _ => {
                // Imports inside `if __TRYKE_TESTING__:` participate in the
                // static import graph so `--changed` mode can precisely
                // re-run in-source tests when their dependencies change.
                if let Some(inner) = testing_guard_body(stmt) {
                    collect_local_import_candidate_groups(root, src_roots, file, inner, groups);
                }
            }
        }
    }
}

/// Extract local file imports from a pre-parsed Python module body.
/// Returns absolute paths of project-local files that this file imports,
/// filtering candidates by on-disk existence. Kept for test
/// compatibility — production discovery uses
/// `extract_local_import_candidate_groups` plus
/// `resolve_import_candidate_groups` to avoid per-import syscalls.
#[cfg(test)]
pub(crate) fn extract_local_imports(
    root: &Path,
    src_roots: &[PathBuf],
    file: &Path,
    body: &[Stmt],
) -> Vec<PathBuf> {
    let groups = extract_local_import_candidate_groups(root, src_roots, file, body);
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for group in &groups {
        for candidate in group {
            if candidate.exists() {
                if seen.insert(candidate.clone()) {
                    out.push(candidate.clone());
                }
                break;
            }
        }
    }
    out
}

/// If `value` is a list literal of string literals AND `targets` is the single
/// name `__lazy_modules__`, return the list contents as borrowed strings.
fn lazy_modules_targets<'a>(targets: &[Expr], value: &'a Expr) -> Option<Vec<&'a str>> {
    let [Expr::Name(n)] = targets else {
        return None;
    };
    if n.id.as_str() != "__lazy_modules__" {
        return None;
    }
    string_list_entries(value)
}

/// `__lazy_modules__: list[str] = [...]` annotated-assignment variant.
fn lazy_modules_ann_target<'a>(target: &Expr, value: &'a Expr) -> Option<Vec<&'a str>> {
    let Expr::Name(n) = target else {
        return None;
    };
    if n.id.as_str() != "__lazy_modules__" {
        return None;
    }
    string_list_entries(value)
}

fn string_list_entries(value: &Expr) -> Option<Vec<&str>> {
    let Expr::List(list) = value else {
        return None;
    };
    list.elts
        .iter()
        .map(|elt| match elt {
            Expr::StringLiteral(s) => Some(s.value.to_str()),
            _ => None,
        })
        .collect()
}

/// For each name in `__lazy_modules__`, push a candidate group resolving
/// it as a sibling submodule (or subpackage) of the declaring file's
/// directory. Each group preserves Python's `name.py` before
/// `name/__init__.py` precedence.
fn push_lazy_module_sibling_groups(
    root: &Path,
    file: &Path,
    names: &[&str],
    groups: &mut Vec<ImportCandidateGroup>,
) {
    let Some(base) = file.parent() else {
        return;
    };
    for name in names {
        let candidates = candidate_relative_import_paths(root, base, name);
        if !candidates.is_empty() {
            groups.push(candidates);
        }
    }
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

const MARKER_ATTRS: &[&str] = &["skip", "todo", "xfail", "skip_if"];

/// Tryke symbols that can be imported directly with `from tryke import …`.
/// Used to bound the `symbol_aliases` table so unrelated imports don't
/// pollute it.
const TRYKE_SYMBOLS: &[&str] = &["describe", "test", "fixture", "Depends"];

/// Per-file table of local names that refer to tryke module / symbols.
///
/// Built once from the parsed module body so discovery matchers can
/// recognise both `import tryke as t` (module alias) and
/// `from tryke import describe as d` (symbol alias) without re-walking
/// the import statements on every match.
#[derive(Default)]
struct TrykeAliases {
    /// Local names bound to the `tryke` module.
    /// Contains "tryke" after `import tryke`; "t" after `import tryke as t`.
    module_aliases: std::collections::HashSet<String>,
    /// Local name → canonical tryke symbol name.
    /// `{"describe": "describe"}` after `from tryke import describe`;
    /// `{"d": "describe"}` after `from tryke import describe as d`.
    symbol_aliases: std::collections::HashMap<String, &'static str>,
}

impl TrykeAliases {
    fn collect(body: &[Stmt]) -> Self {
        let mut out = Self::default();
        // Seed with the canonical module name so qualified `tryke.describe`,
        // `tryke.test`, etc. remain recognised even in files with no visible
        // `import tryke` — this mirrors the bare-symbol legacy fallback
        // (`name == canon` in `is_bare_tryke_symbol`) and keeps synthetic
        // snippets working.
        out.module_aliases.insert("tryke".to_owned());
        out.walk(body);
        out
    }

    fn walk(&mut self, body: &[Stmt]) {
        for stmt in body {
            match stmt {
                Stmt::Import(s) => {
                    for alias in &s.names {
                        if alias.name.id.as_str() == "tryke" {
                            let local = alias.asname.as_ref().map_or("tryke", |n| n.id.as_str());
                            self.module_aliases.insert(local.to_owned());
                        }
                    }
                }
                Stmt::ImportFrom(s)
                    if s.level == 0
                        && s.module.as_ref().is_some_and(|m| m.id.as_str() == "tryke") =>
                {
                    for alias in &s.names {
                        let name = alias.name.id.as_str();
                        if let Some(canon) = TRYKE_SYMBOLS.iter().find(|s| **s == name) {
                            let local = alias.asname.as_ref().map_or(name, |n| n.id.as_str());
                            self.symbol_aliases.insert(local.to_owned(), canon);
                        }
                    }
                }
                _ => {
                    if let Some(inner) = testing_guard_body(stmt) {
                        self.walk(inner);
                    }
                }
            }
        }
    }

    /// Is `name` a local reference to the `tryke` module?
    fn is_module(&self, name: &str) -> bool {
        self.module_aliases.contains(name)
    }

    /// Does `name` resolve to the tryke symbol `canon`?
    fn is_symbol(&self, name: &str, canon: &str) -> bool {
        self.symbol_aliases.get(name).copied() == Some(canon)
    }
}

/// Returns `true` if `expr` is a `@test.cases(...)` call (bare or qualified).
/// Must be a `Call` expression — the bare `test.cases` attribute form has no
/// runtime meaning.
fn is_tryke_test_cases_decorator(expr: &Expr, body: &[Stmt], aliases: &TrykeAliases) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Expr::Attribute(attr) = &*call.func else {
        return false;
    };
    if attr.attr.id.as_str() != "cases" {
        return false;
    }
    is_bare_test_or_qualified(&attr.value, body, aliases)
}

/// Recognises bare `test` / `tryke.test` plus the marker attribute forms
/// (`test.skip`, `test.xfail`, …) and their call wrappers.
fn is_tryke_test_decorator(expr: &Expr, body: &[Stmt], aliases: &TrykeAliases) -> bool {
    match expr {
        // tryke.test (or any module alias of tryke)
        Expr::Attribute(a) if a.attr.id.as_str() == "test" => {
            matches!(&*a.value, Expr::Name(n) if aliases.is_module(n.id.as_str()))
        }
        // test.skip, test.todo, test.xfail, test.skip_if
        Expr::Attribute(a) if MARKER_ATTRS.contains(&a.attr.id.as_str()) => {
            is_bare_test_or_qualified(&a.value, body, aliases)
        }
        // Bare test (possibly via `from tryke import test as X`)
        Expr::Name(n) => is_bare_tryke_symbol(n.id.as_str(), "test", body, aliases),
        // Call wrapper: @test(), @test.skip("reason"), @test("name"), etc.
        Expr::Call(c) => is_tryke_test_decorator(&c.func, body, aliases),
        _ => false,
    }
}

/// Returns true for `test` (Name) or `tryke.test` (Attribute).
fn is_bare_test_or_qualified(expr: &Expr, body: &[Stmt], aliases: &TrykeAliases) -> bool {
    match expr {
        Expr::Name(n) => is_bare_tryke_symbol(n.id.as_str(), "test", body, aliases),
        Expr::Attribute(a) => {
            a.attr.id.as_str() == "test"
                && matches!(&*a.value, Expr::Name(n) if aliases.is_module(n.id.as_str()))
        }
        _ => false,
    }
}

/// Resolve a bare name to a canonical tryke symbol.
///
/// Matches when either (a) the name is explicitly aliased to `canon` via
/// `from tryke import <canon> [as <name>]`, or (b) the name is literally
/// `canon` and not shadowed by a local definition — the legacy heuristic
/// that keeps working for files with no visible import.
fn is_bare_tryke_symbol(name: &str, canon: &str, body: &[Stmt], aliases: &TrykeAliases) -> bool {
    if is_locally_defined(name, body) {
        return false;
    }
    aliases.is_symbol(name, canon) || name == canon
}

/// Check whether a decorator is the tryke `@fixture` decorator. Returns the
/// fixture's `per` granularity (`Test` or `Scope`) if it is, `None` otherwise.
///
/// Recognises all four forms:
/// - `@fixture`
/// - `@fixture()`
/// - `@fixture(per="scope")`
/// - `@tryke.fixture` / `@tryke.fixture(per="scope")`
fn is_tryke_fixture_decorator(
    expr: &Expr,
    body: &[Stmt],
    aliases: &TrykeAliases,
) -> Option<FixturePer> {
    match expr {
        // Bare name: @fixture (or alias from `from tryke import fixture as X`)
        Expr::Name(n) if is_bare_tryke_symbol(n.id.as_str(), "fixture", body, aliases) => {
            Some(FixturePer::Test)
        }
        // Qualified: @tryke.fixture (or any module alias of tryke)
        Expr::Attribute(a)
            if a.attr.id.as_str() == "fixture"
                && matches!(&*a.value, Expr::Name(n) if aliases.is_module(n.id.as_str())) =>
        {
            Some(FixturePer::Test)
        }
        // Call wrapper: @fixture(...) or @tryke.fixture(...)
        Expr::Call(c) => {
            let base = is_tryke_fixture_decorator(&c.func, body, aliases)?;
            // Inspect keyword arguments for `per="scope"`.
            for kw in &c.arguments.keywords {
                if kw.arg.as_ref().is_some_and(|k| k.id.as_str() == "per")
                    && let Expr::StringLiteral(s) = &kw.value
                {
                    return match s.value.to_str() {
                        "test" => Some(FixturePer::Test),
                        "scope" => Some(FixturePer::Scope),
                        // Unknown values fall through to the default; users
                        // see a typed error at worker registration time.
                        _ => Some(base),
                    };
                }
            }
            Some(base)
        }
        _ => None,
    }
}

/// Extract function names from `Depends(name)` calls in parameter defaults.
///
/// Malformed `Depends(...)` forms (attribute access, calls, no argument)
/// push a human-readable diagnostic into `errors` rather than being
/// silently dropped — at runtime they would cause cryptic `TypeError`s
/// on missing kwargs, so we surface them at discovery time.
fn extract_depends_from_params(
    func: &ruff_python_ast::StmtFunctionDef,
    file: &Path,
    root: &Path,
    line_index: &LineIndex,
    top_body: &[Stmt],
    aliases: &TrykeAliases,
    errors: &mut Vec<String>,
) -> Vec<String> {
    let mut deps = Vec::new();
    for param in func.parameters.iter_non_variadic_params() {
        let Some(default) = &param.default else {
            continue;
        };
        let Expr::Call(call) = default.as_ref() else {
            continue;
        };
        let is_depends = match call.func.as_ref() {
            Expr::Name(n) => is_bare_tryke_symbol(n.id.as_str(), "Depends", top_body, aliases),
            Expr::Attribute(a) => {
                a.attr.id.as_str() == "Depends"
                    && matches!(&*a.value, Expr::Name(n) if aliases.is_module(n.id.as_str()))
            }
            _ => false,
        };
        if !is_depends {
            continue;
        }
        let line = u32::try_from(line_index.line_index(call.range.start()).get()).unwrap_or(0);
        let display_file = file.strip_prefix(root).unwrap_or(file).display();
        let param_name = param.parameter.name.id.as_str();
        match call.arguments.args.first() {
            None => {
                errors.push(format!(
                    "{display_file}:{line}: Depends() in parameter '{param_name}' of \
                     '{fn_name}' requires exactly one positional argument naming \
                     the hook function, e.g. Depends(my_hook).",
                    fn_name = func.name.id.as_str(),
                ));
            }
            Some(Expr::Name(arg)) => {
                deps.push(arg.id.as_str().to_owned());
            }
            Some(other) => {
                errors.push(format!(
                    "{display_file}:{line}: Depends({kind}) in parameter '{param_name}' of \
                     '{fn_name}' is not supported — only bare function name references \
                     are allowed (e.g. Depends(my_hook), not Depends(mod.hook) or \
                     Depends(factory())).",
                    kind = describe_expr_kind(other),
                    fn_name = func.name.id.as_str(),
                ));
            }
        }
    }
    deps
}

/// Short, human-readable label for an AST expression kind. Used in
/// diagnostic messages so users can tell what shape of `Depends(...)`
/// argument was rejected.
fn describe_expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Name(_) => "name",
        Expr::Attribute(_) => "attribute",
        Expr::Call(_) => "call",
        Expr::Subscript(_) => "subscript",
        Expr::Lambda(_) => "lambda",
        Expr::StringLiteral(_) => "string",
        Expr::NumberLiteral(_) => "number",
        _ => "expression",
    }
}

/// What kind of lifecycle modifier is on the `@test` decorator?
#[derive(Debug, Clone, PartialEq)]
pub enum TestModifier {
    None,
    Skip(String),
    Todo(String),
    Xfail(String),
    SkipIf,
}

/// Per-case metadata extracted from a `test.case(...)` call.
///
/// The typed form supports `skip`, `xfail`, and `todo` as reserved keyword
/// arguments. Non-literal values are silently treated as `None` because
/// discovery cannot evaluate them — the Python worker handles those at
/// runtime as a fallback.
#[derive(Debug, Clone, Default)]
struct CaseInfo {
    label: String,
    skip: Option<String>,
    xfail: Option<String>,
    todo: Option<String>,
}

/// Walk through Call / Attribute layers to extract the modifier.
/// - `@test`              → None
/// - `@test.skip`         → Skip("")
/// - `@test.skip("r")`    → Skip("r")
fn extract_test_modifier(expr: &Expr) -> TestModifier {
    match expr {
        Expr::Attribute(a) if MARKER_ATTRS.contains(&a.attr.id.as_str()) => {
            match a.attr.id.as_str() {
                "skip" => TestModifier::Skip(String::new()),
                "todo" => TestModifier::Todo(String::new()),
                "xfail" => TestModifier::Xfail(String::new()),
                "skip_if" => TestModifier::SkipIf,
                _ => TestModifier::None,
            }
        }
        Expr::Attribute(a) if a.attr.id.as_str() == "test" => TestModifier::None,
        Expr::Call(c) => {
            let base = extract_test_modifier(&c.func);
            match base {
                TestModifier::Skip(_) => TestModifier::Skip(extract_first_string_arg(c)),
                TestModifier::Todo(_) => TestModifier::Todo(extract_first_string_arg(c)),
                TestModifier::Xfail(_) => TestModifier::Xfail(extract_first_string_arg(c)),
                // @test("name") or @test(name="foo") — still plain
                TestModifier::None => TestModifier::None,
                other @ TestModifier::SkipIf => other,
            }
        }
        _ => TestModifier::None,
    }
}

/// Extract the first positional string arg or `reason=`/`description=` kwarg.
fn extract_first_string_arg(call: &ruff_python_ast::ExprCall) -> String {
    for kw in &call.arguments.keywords {
        if let Some(k) = kw.arg.as_ref() {
            let key = k.id.as_str();
            if (key == "reason" || key == "description")
                && let Expr::StringLiteral(s) = &kw.value
            {
                return s.value.to_str().to_owned();
            }
        }
    }
    if let Some(first) = call.arguments.args.first()
        && let Expr::StringLiteral(s) = first
    {
        return s.value.to_str().to_owned();
    }
    String::new()
}

/// Extract `tags=[...]` kwarg from any call-form decorator.
fn extract_decorator_tags(expr: &Expr) -> Vec<String> {
    let Expr::Call(call) = expr else {
        return vec![];
    };
    for kw in &call.arguments.keywords {
        if kw.arg.as_ref().is_some_and(|k| k.id.as_str() == "tags")
            && let Expr::List(list) = &kw.value
        {
            return list
                .elts
                .iter()
                .filter_map(|e| {
                    if let Expr::StringLiteral(s) = e {
                        Some(s.value.to_str().to_owned())
                    } else {
                        Option::None
                    }
                })
                .collect();
        }
    }
    vec![]
}

/// Returns `true` if `expr` is a call to `test.case(...)` or `tryke.test.case(...)`.
fn is_test_case_call(expr: &Expr, body: &[Stmt], aliases: &TrykeAliases) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Expr::Attribute(attr) = &*call.func else {
        return false;
    };
    if attr.attr.id.as_str() != "case" {
        return false;
    }
    match &*attr.value {
        Expr::Name(n) => is_bare_tryke_symbol(n.id.as_str(), "test", body, aliases),
        Expr::Attribute(a) => {
            a.attr.id.as_str() == "test"
                && matches!(&*a.value, Expr::Name(n) if aliases.is_module(n.id.as_str()))
        }
        _ => false,
    }
}

/// Extract a named string-literal keyword argument from a call's keywords.
fn extract_string_kwarg(keywords: &[ruff_python_ast::Keyword], name: &str) -> Option<String> {
    keywords.iter().find_map(|kw| {
        if kw.arg.as_ref().is_some_and(|k| k.id.as_str() == name)
            && let Expr::StringLiteral(s) = &kw.value
        {
            Some(s.value.to_str().to_owned())
        } else {
            Option::None
        }
    })
}

/// Extract per-case modifiers (`skip`, `xfail`, `todo`) from a `test.case(...)` call.
fn extract_case_modifiers(call: &ruff_python_ast::ExprCall) -> CaseInfo {
    CaseInfo {
        label: String::new(),
        skip: extract_string_kwarg(&call.arguments.keywords, "skip"),
        xfail: extract_string_kwarg(&call.arguments.keywords, "xfail"),
        todo: extract_string_kwarg(&call.arguments.keywords, "todo"),
    }
}

/// Extract case info from a `@test.cases(...)` decorator.
///
/// Supports three literal forms:
/// - typed: `@test.cases(test.case("label1", ...), test.case("label2", ...))`
///   — each positional argument is a `test.case(...)` call with a string
///   literal label as its first positional argument. Per-case `skip`, `xfail`,
///   and `todo` keyword arguments are extracted when they are string literals.
/// - kwargs: `@test.cases(zero={...}, one={...})` — each keyword name is a label
/// - list: `@test.cases([("label1", {...}), ("label2", {...})])` — first tuple
///   element is a string-literal label
///
/// Returns `Err(msg)` if the decorator's shape is not statically recognizable
/// (e.g. `@test.cases(build())` or `@test.cases([dynamic, ...])`).
fn extract_cases(
    expr: &Expr,
    body: &[Stmt],
    aliases: &TrykeAliases,
) -> Result<Vec<CaseInfo>, String> {
    let Expr::Call(call) = expr else {
        return Err("test.cases decorator must be called, e.g. @test.cases(a=...)".to_owned());
    };

    let has_args = !call.arguments.args.is_empty();
    let has_kwargs = !call.arguments.keywords.is_empty();

    if has_args && has_kwargs {
        return Err(
            "test.cases() accepts either positional specs/list or keyword arguments, not both"
                .to_owned(),
        );
    }

    if has_kwargs {
        let mut cases = Vec::with_capacity(call.arguments.keywords.len());
        for kw in &call.arguments.keywords {
            let Some(k) = kw.arg.as_ref() else {
                return Err("test.cases() does not support **kwargs expansion — \
                            all labels must be literal keyword arguments"
                    .to_owned());
            };
            cases.push(CaseInfo {
                label: k.id.as_str().to_owned(),
                ..CaseInfo::default()
            });
        }
        return Ok(cases);
    }

    if has_args {
        // Typed form: every positional arg is a `test.case(...)` call.
        if call
            .arguments
            .args
            .iter()
            .all(|a| is_test_case_call(a, body, aliases))
        {
            let mut cases = Vec::with_capacity(call.arguments.args.len());
            for (i, elt) in call.arguments.args.iter().enumerate() {
                let Expr::Call(inner) = elt else {
                    return Err(format!(
                        "test.cases() positional arg {i} must be a test.case(...) call"
                    ));
                };
                let Some(first) = inner.arguments.args.first() else {
                    return Err(format!(
                        "test.cases() positional arg {i}: test.case() requires a label"
                    ));
                };
                let Expr::StringLiteral(s) = first else {
                    return Err(format!(
                        "test.cases() positional arg {i}: test.case() label must be a string literal"
                    ));
                };
                let mut info = extract_case_modifiers(inner);
                s.value.to_str().clone_into(&mut info.label);
                cases.push(info);
            }
            return Ok(cases);
        }

        if call.arguments.args.len() != 1 {
            return Err("test.cases() positional form takes exactly one list argument".to_owned());
        }
        let Expr::List(list) = &call.arguments.args[0] else {
            return Err(
                "test.cases() positional argument must be a list literal of (label, args) tuples \
                 or a sequence of test.case(...) calls"
                    .to_owned(),
            );
        };
        let mut cases = Vec::with_capacity(list.elts.len());
        for (i, elt) in list.elts.iter().enumerate() {
            let Expr::Tuple(tup) = elt else {
                return Err(format!(
                    "test.cases() list element {i} must be a (label, args) tuple literal"
                ));
            };
            let Some(first) = tup.elts.first() else {
                return Err(format!(
                    "test.cases() list element {i} tuple must have a label as its first element"
                ));
            };
            let Expr::StringLiteral(s) = first else {
                return Err(format!(
                    "test.cases() list element {i} label must be a string literal"
                ));
            };
            cases.push(CaseInfo {
                label: s.value.to_str().to_owned(),
                ..CaseInfo::default()
            });
        }
        return Ok(cases);
    }

    Err("test.cases() requires at least one case".to_owned())
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

/// Returns `true` if any expression in the tree is a dynamic import call:
/// `importlib.import_module(...)` or `__import__(...)`.
fn expr_has_dynamic_import(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => {
            let is_dynamic = match call.func.as_ref() {
                // __import__(...)
                Expr::Name(n) => n.id.as_str() == "__import__",
                // importlib.import_module(...)
                Expr::Attribute(a) => {
                    a.attr.id.as_str() == "import_module"
                        && matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "importlib")
                }
                _ => false,
            };
            if is_dynamic {
                return true;
            }
            expr_has_dynamic_import(&call.func)
                || call.arguments.args.iter().any(expr_has_dynamic_import)
        }
        _ => false,
    }
}

fn stmt_has_dynamic_import(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(s) => expr_has_dynamic_import(&s.value),
        Stmt::Return(s) => s.value.as_ref().is_some_and(|v| expr_has_dynamic_import(v)),
        Stmt::Assign(s) => expr_has_dynamic_import(&s.value),
        Stmt::AnnAssign(s) => s.value.as_ref().is_some_and(|v| expr_has_dynamic_import(v)),
        Stmt::FunctionDef(f) => f.body.iter().any(stmt_has_dynamic_import),
        Stmt::If(s) => {
            // `if __TRYKE_TESTING__:` is unreachable in production, so a
            // dynamic import inside must not mark the file always-dirty.
            // `testing_guard_body` requires an empty elif/else, so if it
            // matches the body is the only branch to skip.
            if testing_guard_body(stmt).is_some() {
                return false;
            }
            s.body.iter().any(stmt_has_dynamic_import)
                || s.elif_else_clauses
                    .iter()
                    .any(|c| c.body.iter().any(stmt_has_dynamic_import))
        }
        Stmt::For(s) => s
            .body
            .iter()
            .chain(s.orelse.iter())
            .any(stmt_has_dynamic_import),
        Stmt::While(s) => s
            .body
            .iter()
            .chain(s.orelse.iter())
            .any(stmt_has_dynamic_import),
        Stmt::With(s) => s.body.iter().any(stmt_has_dynamic_import),
        Stmt::Try(s) => s
            .body
            .iter()
            .chain(s.orelse.iter())
            .chain(s.finalbody.iter())
            .any(stmt_has_dynamic_import),
        _ => false,
    }
}

/// Returns `true` if the module body contains any dynamic import calls
/// (`importlib.import_module(...)` or `__import__(...)`).
pub(crate) fn has_dynamic_imports(body: &[Stmt]) -> bool {
    body.iter().any(stmt_has_dynamic_import)
}

/// Collect 1-indexed source lines of any `if __TRYKE_TESTING__:` statement
/// that has an `elif` or `else` branch. These shapes are silently dropped by
/// `testing_guard_body`, so we record them to surface a warning.
pub(crate) fn find_testing_guard_else_lines(body: &[Stmt], line_index: &LineIndex) -> Vec<u32> {
    let mut out = Vec::new();
    collect_testing_guard_else_lines(body, line_index, &mut out);
    out
}

fn collect_testing_guard_else_lines(body: &[Stmt], line_index: &LineIndex, out: &mut Vec<u32>) {
    for stmt in body {
        if let Stmt::If(s) = stmt
            && is_testing_guard_condition(&s.test)
            && !s.elif_else_clauses.is_empty()
        {
            let line = u32::try_from(line_index.line_index(s.range.start()).get()).unwrap_or(1);
            out.push(line);
        }
        // Recurse into nested bodies so a guard-with-else inside a function,
        // class, describe(), or another if-block is still reported.
        match stmt {
            Stmt::If(s) => {
                collect_testing_guard_else_lines(&s.body, line_index, out);
                for c in &s.elif_else_clauses {
                    collect_testing_guard_else_lines(&c.body, line_index, out);
                }
            }
            Stmt::With(s) => collect_testing_guard_else_lines(&s.body, line_index, out),
            Stmt::For(s) => {
                collect_testing_guard_else_lines(&s.body, line_index, out);
                collect_testing_guard_else_lines(&s.orelse, line_index, out);
            }
            Stmt::While(s) => {
                collect_testing_guard_else_lines(&s.body, line_index, out);
                collect_testing_guard_else_lines(&s.orelse, line_index, out);
            }
            Stmt::FunctionDef(f) => collect_testing_guard_else_lines(&f.body, line_index, out),
            Stmt::ClassDef(c) => collect_testing_guard_else_lines(&c.body, line_index, out),
            Stmt::Try(s) => {
                collect_testing_guard_else_lines(&s.body, line_index, out);
                collect_testing_guard_else_lines(&s.orelse, line_index, out);
                collect_testing_guard_else_lines(&s.finalbody, line_index, out);
            }
            _ => {}
        }
    }
}

/// If `stmt` is `if __TRYKE_TESTING__:` or `if tryke_guard.__TRYKE_TESTING__:`
/// with no elif/else clauses, return its body. Otherwise, return `None`.
///
/// This is the canonical "in-source testing guard" pattern: code inside the
/// block is executed only when `tryke_guard.__TRYKE_TESTING__` is truthy (i.e.
/// under the tryke worker). Discovery descends into matched guards to pick up
/// tests / fixtures / doctests / imports, and treats dynamic imports inside
/// the guard as unreachable in production (so they do not flag the file
/// always-dirty).
///
/// Narrow by design: no negation, no elif/else, no compound conditions, no
/// aliases. Shapes with elif/else get detected separately by
/// `is_testing_guard_with_else` so we can surface a warning rather than
/// silently dropping tests.
fn testing_guard_body(stmt: &Stmt) -> Option<&[Stmt]> {
    let Stmt::If(s) = stmt else {
        return None;
    };
    if !s.elif_else_clauses.is_empty() {
        return None;
    }
    if !is_testing_guard_condition(&s.test) {
        return None;
    }
    Some(&s.body)
}

/// Returns `true` when `expr` is the bare name `__TRYKE_TESTING__` or the
/// attribute `tryke_guard.__TRYKE_TESTING__`.
fn is_testing_guard_condition(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "__TRYKE_TESTING__",
        Expr::Attribute(a) => {
            a.attr.id.as_str() == "__TRYKE_TESTING__"
                && matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "tryke_guard")
        }
        _ => false,
    }
}

/// Check whether an expression is a call to `describe` (bare or `tryke.describe`).
/// Returns the describe name if it is, `None` otherwise.
fn extract_describe_name(expr: &Expr, body: &[Stmt], aliases: &TrykeAliases) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let is_describe = match call.func.as_ref() {
        Expr::Name(n) => is_bare_tryke_symbol(n.id.as_str(), "describe", body, aliases),
        Expr::Attribute(a) => {
            a.attr.id.as_str() == "describe"
                && matches!(&*a.value, Expr::Name(n) if aliases.is_module(n.id.as_str()))
        }
        _ => return None,
    };
    if !is_describe {
        return None;
    }
    if let Some(first) = call.arguments.args.first()
        && let Expr::StringLiteral(s) = first
    {
        return Some(s.value.to_str().to_owned());
    }
    // Also accept the kwarg form: `describe(name="…")`.
    for kw in &call.arguments.keywords {
        if kw.arg.as_ref().is_some_and(|k| k.id.as_str() == "name")
            && let Expr::StringLiteral(s) = &kw.value
        {
            return Some(s.value.to_str().to_owned());
        }
    }
    None
}

#[expect(clippy::too_many_arguments)]
fn collect_cases_from_func(
    func: &ruff_python_ast::StmtFunctionDef,
    cases_dec: &ruff_python_ast::Decorator,
    top_body: &[Stmt],
    root: &Path,
    file: &Path,
    source: &str,
    line_index: &LineIndex,
    aliases: &TrykeAliases,
    groups: &[String],
    tests_out: &mut Vec<TestItem>,
    errors_out: &mut Vec<String>,
) {
    // Forbid `@test` and `@test.cases` on the same function — the runtime
    // dispatch can only resolve one of them.
    let plain_test_dec = func.decorator_list.iter().any(|d| {
        is_tryke_test_decorator(&d.expression, top_body, aliases)
            && !is_tryke_test_cases_decorator(&d.expression, top_body, aliases)
            && matches!(extract_test_modifier(&d.expression), TestModifier::None)
    });
    if plain_test_dec {
        let display_file = file.strip_prefix(root).unwrap_or(file).display();
        let line = u32::try_from(line_index.line_index(func.range.start()).get()).unwrap_or(0);
        errors_out.push(format!(
            "{display_file}:{line}: function '{fn_name}' has both '@test' and \
             '@test.cases' — use one or the other",
            fn_name = func.name.id.as_str(),
        ));
        return;
    }

    let cases = match extract_cases(&cases_dec.expression, top_body, aliases) {
        Ok(cases) => cases,
        Err(msg) => {
            let display_file = file.strip_prefix(root).unwrap_or(file).display();
            let line = u32::try_from(line_index.line_index(func.range.start()).get()).unwrap_or(0);
            errors_out.push(format!(
                "{display_file}:{line}: @test.cases on '{fn_name}': {msg}",
                fn_name = func.name.id.as_str(),
            ));
            return;
        }
    };

    // Function-level modifiers (@test.skip / @test.xfail / @test.todo) act as
    // defaults — per-case modifiers from test.case(...) take precedence.
    let modifier_dec = func.decorator_list.iter().find(|d| {
        is_tryke_test_decorator(&d.expression, top_body, aliases)
            && !is_tryke_test_cases_decorator(&d.expression, top_body, aliases)
            && !matches!(extract_test_modifier(&d.expression), TestModifier::None)
    });
    let modifier =
        modifier_dec.map_or(TestModifier::None, |d| extract_test_modifier(&d.expression));
    let (fn_skip, fn_todo, fn_xfail) = match modifier {
        TestModifier::Skip(r) => (Some(r), None, None),
        TestModifier::Todo(d) => (None, Some(d), None),
        TestModifier::Xfail(r) => (None, None, Some(r)),
        TestModifier::SkipIf | TestModifier::None => (None, None, None),
    };

    let display_name = extract_docstring(&func.body);
    let line_number = u32::try_from(line_index.line_index(func.range.start()).get()).ok();
    let file_path = Some(file.strip_prefix(root).unwrap_or(file).to_path_buf());
    let module_path = path_to_module(root, file);
    let expected_assertions = extract_expected_assertions(&func.body, source, line_index);

    for (i, case) in cases.into_iter().enumerate() {
        tests_out.push(TestItem {
            name: func.name.id.as_str().to_owned(),
            module_path: module_path.clone(),
            file_path: file_path.clone(),
            line_number,
            display_name: display_name.clone(),
            expected_assertions: expected_assertions.clone(),
            skip: case.skip.or_else(|| fn_skip.clone()),
            todo: case.todo.or_else(|| fn_todo.clone()),
            xfail: case.xfail.or_else(|| fn_xfail.clone()),
            tags: vec![],
            groups: groups.to_vec(),
            case_label: Some(case.label),
            case_index: u32::try_from(i).ok(),
            ..TestItem::default()
        });
    }
}

#[expect(clippy::too_many_arguments)]
fn collect_tests_from_body(
    stmts: &[Stmt],
    top_body: &[Stmt],
    root: &Path,
    file: &Path,
    source: &str,
    line_index: &LineIndex,
    aliases: &TrykeAliases,
    groups: &[String],
    tests_out: &mut Vec<TestItem>,
    hooks_out: &mut Vec<HookItem>,
    errors_out: &mut Vec<String>,
) {
    for stmt in stmts {
        if let Stmt::FunctionDef(func) = stmt {
            // `@test.cases(...)` and `@test` (or its marker forms) live on
            // different sub-paths. `@test.cases` emits N items per function;
            // the plain `@test` path emits exactly one.
            let cases_dec = func
                .decorator_list
                .iter()
                .find(|d| is_tryke_test_cases_decorator(&d.expression, top_body, aliases));
            let test_dec = func
                .decorator_list
                .iter()
                .find(|d| is_tryke_test_decorator(&d.expression, top_body, aliases));

            if let Some(cases_dec) = cases_dec {
                collect_cases_from_func(
                    func, cases_dec, top_body, root, file, source, line_index, aliases, groups,
                    tests_out, errors_out,
                );
            } else if let Some(dec) = test_dec {
                let display_name = extract_decorator_name(&dec.expression)
                    .or_else(|| extract_docstring(&func.body));
                let modifier = extract_test_modifier(&dec.expression);
                let tags = extract_decorator_tags(&dec.expression);
                let (skip, todo, xfail) = match modifier {
                    TestModifier::Skip(r) => (Some(r), None, None),
                    TestModifier::Todo(d) => (None, Some(d), None),
                    TestModifier::Xfail(r) => (None, None, Some(r)),
                    TestModifier::SkipIf | TestModifier::None => (None, None, None),
                };
                tests_out.push(TestItem {
                    name: func.name.id.as_str().to_owned(),
                    module_path: path_to_module(root, file),
                    file_path: Some(file.strip_prefix(root).unwrap_or(file).to_path_buf()),
                    line_number: u32::try_from(line_index.line_index(func.range.start()).get())
                        .ok(),
                    display_name,
                    expected_assertions: extract_expected_assertions(
                        &func.body, source, line_index,
                    ),
                    skip,
                    todo,
                    xfail,
                    tags,
                    groups: groups.to_vec(),
                    ..TestItem::default()
                });
            }
            // Check for @fixture decorator
            else if let Some(per) = func
                .decorator_list
                .iter()
                .find_map(|d| is_tryke_fixture_decorator(&d.expression, top_body, aliases))
            {
                let depends_on = extract_depends_from_params(
                    func, file, root, line_index, top_body, aliases, errors_out,
                );
                hooks_out.push(HookItem {
                    name: func.name.id.as_str().to_owned(),
                    module_path: path_to_module(root, file),
                    per,
                    groups: groups.to_vec(),
                    depends_on,
                    line_number: u32::try_from(line_index.line_index(func.range.start()).get())
                        .ok(),
                });
            }
        } else if let Stmt::With(with_stmt) = stmt {
            // Check if this is a `with describe("name")` block
            let describe_name = with_stmt
                .items
                .iter()
                .find_map(|item| extract_describe_name(&item.context_expr, top_body, aliases));
            if let Some(name) = describe_name {
                let mut nested_groups = groups.to_vec();
                nested_groups.push(name);
                collect_tests_from_body(
                    &with_stmt.body,
                    top_body,
                    root,
                    file,
                    source,
                    line_index,
                    aliases,
                    &nested_groups,
                    tests_out,
                    hooks_out,
                    errors_out,
                );
            }
        } else if let Some(inner) = testing_guard_body(stmt) {
            // `if __TRYKE_TESTING__:` block — recurse with the same top_body
            // so decorator / fixture / describe resolution still sees
            // module-level imports, and with the same groups so tests inside
            // the guard keep their enclosing describe() context.
            collect_tests_from_body(
                inner, top_body, root, file, source, line_index, aliases, groups, tests_out,
                hooks_out, errors_out,
            );
        }
    }
}

/// Returns `true` if the first statement in `body` is a string literal
/// whose text contains `>>>` (i.e. a docstring with doctest examples).
fn has_doctest_in_docstring(body: &[Stmt]) -> bool {
    if let Some(Stmt::Expr(s)) = body.first()
        && let Expr::StringLiteral(lit) = &*s.value
    {
        return lit.value.to_str().contains(">>>");
    }
    false
}

/// Walk the module body and emit a [`TestItem`] for every object whose
/// docstring contains `>>>` examples.
fn collect_doctests_from_body(
    stmts: &[Stmt],
    root: &Path,
    file: &Path,
    line_index: &LineIndex,
    prefix: &str,
    out: &mut Vec<TestItem>,
) {
    // Module-level docstring (only when prefix is empty, i.e. top-level call).
    if prefix.is_empty()
        && has_doctest_in_docstring(stmts)
        && let Some(Stmt::Expr(s)) = stmts.first()
    {
        let line = u32::try_from(line_index.line_index(s.range.start()).get()).unwrap_or(1);
        out.push(TestItem {
            name: "__module__".to_owned(),
            module_path: path_to_module(root, file),
            file_path: Some(file.strip_prefix(root).unwrap_or(file).to_path_buf()),
            line_number: Some(line),
            display_name: Some("doctest: (module)".to_owned()),
            doctest_object: Some(String::new()),
            ..TestItem::default()
        });
    }

    for stmt in stmts {
        match stmt {
            Stmt::FunctionDef(func) => {
                if has_doctest_in_docstring(&func.body) {
                    let object_path = if prefix.is_empty() {
                        func.name.id.as_str().to_owned()
                    } else {
                        format!("{prefix}.{}", func.name.id.as_str())
                    };
                    let line =
                        u32::try_from(line_index.line_index(func.range.start()).get()).unwrap_or(1);
                    out.push(TestItem {
                        name: object_path.clone(),
                        module_path: path_to_module(root, file),
                        file_path: Some(file.strip_prefix(root).unwrap_or(file).to_path_buf()),
                        line_number: Some(line),
                        display_name: Some(format!("doctest: {object_path}")),
                        doctest_object: Some(object_path),
                        ..TestItem::default()
                    });
                }
            }
            Stmt::ClassDef(class) => {
                let class_name = if prefix.is_empty() {
                    class.name.id.as_str().to_owned()
                } else {
                    format!("{prefix}.{}", class.name.id.as_str())
                };

                // Class-level docstring
                if has_doctest_in_docstring(&class.body) {
                    let line = u32::try_from(line_index.line_index(class.range.start()).get())
                        .unwrap_or(1);
                    out.push(TestItem {
                        name: class_name.clone(),
                        module_path: path_to_module(root, file),
                        file_path: Some(file.strip_prefix(root).unwrap_or(file).to_path_buf()),
                        line_number: Some(line),
                        display_name: Some(format!("doctest: {class_name}")),
                        doctest_object: Some(class_name.clone()),
                        ..TestItem::default()
                    });
                }

                // Recurse into methods
                collect_doctests_from_body(&class.body, root, file, line_index, &class_name, out);
            }
            _ => {
                // `if __TRYKE_TESTING__:` block — doctests on functions/classes
                // inside the guard should be discovered the same as at module
                // top level.
                if let Some(inner) = testing_guard_body(stmt) {
                    collect_doctests_from_body(inner, root, file, line_index, prefix, out);
                }
            }
        }
    }
}

/// Parse `source` once and produce everything discovery needs: the
/// `ParsedFile` (tests, hooks, guard-else lines, errors), the project-local
/// imports this file depends on, and whether it contains dynamic imports.
/// Folding all three derivations into a single AST walk avoids the prior
/// cold-start cost of parsing each file twice.
pub(crate) fn discover_file_from_source(
    root: &Path,
    src_roots: &[PathBuf],
    file: &Path,
    source: &str,
) -> crate::db::DiscoveredFile {
    trace!(
        "parsing {}",
        file.strip_prefix(root).unwrap_or(file).display()
    );
    let Ok(parsed) = parse_module(source) else {
        trace!("parse error in {}", file.display());
        return crate::db::DiscoveredFile::default();
    };
    let line_index = LineIndex::from_source_text(source);
    let module = parsed.syntax();
    let body = &module.body;
    let aliases = TrykeAliases::collect(body);
    let mut tests = Vec::new();
    let mut hooks = Vec::new();
    let mut errors = Vec::new();
    collect_tests_from_body(
        body,
        body,
        root,
        file,
        source,
        &line_index,
        &aliases,
        &[],
        &mut tests,
        &mut hooks,
        &mut errors,
    );
    collect_doctests_from_body(body, root, file, &line_index, "", &mut tests);
    let testing_guard_else_lines = find_testing_guard_else_lines(body, &line_index);
    let import_candidates = extract_local_import_candidate_groups(root, src_roots, file, body);
    let dynamic_imports = has_dynamic_imports(body);
    for err in &errors {
        log::error!("tryke discovery: {err}");
    }
    crate::db::DiscoveredFile {
        parsed: ParsedFile {
            tests,
            hooks,
            testing_guard_else_lines,
            errors,
        },
        import_candidates,
        dynamic_imports,
    }
}

pub(crate) fn parse_tests_from_source(
    root: &Path,
    src_roots: &[PathBuf],
    file: &Path,
    source: &str,
) -> ParsedFile {
    discover_file_from_source(root, src_roots, file, source).parsed
}

fn parse_tests_from_file(root: &Path, src_roots: &[PathBuf], file: &Path) -> ParsedFile {
    let Ok(source) = fs::read_to_string(file) else {
        return ParsedFile::default();
    };
    parse_tests_from_source(root, src_roots, file, &source)
}

#[must_use]
pub fn discover_from(start: &Path) -> Vec<TestItem> {
    let config = tryke_config::load_effective_config(start);
    discover_from_with_options(start, &config.discovery.exclude, &config.discovery.src)
}

#[must_use]
pub fn discover_from_with_excludes(start: &Path, excludes: &[String]) -> Vec<TestItem> {
    let src = tryke_config::load_effective_config(start).discovery.src;
    discover_from_with_options(start, excludes, &src)
}

#[must_use]
pub fn discover_from_with_options(
    start: &Path,
    excludes: &[String],
    src: &[String],
) -> Vec<TestItem> {
    let root = find_project_root(start).unwrap_or_else(|| start.to_path_buf());
    let src_roots = resolve_src_roots(&root, src);
    let mut files = collect_python_files(&root, excludes);
    files.sort();
    let parsed: Vec<ParsedFile> = files
        .par_iter()
        .map(|f| parse_tests_from_file(&root, &src_roots, f))
        .collect();
    let mut tests: Vec<TestItem> = parsed.into_iter().flat_map(|p| p.tests).collect();
    tests.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.line_number.cmp(&b.line_number))
    });
    tests
}

/// Resolve configured `src` entries (relative strings) into absolute
/// source roots under `root`. Canonicalizes each so comparisons against
/// the enumerated project file set (also canonicalized by the
/// discoverer) compare apples to apples; falls back to the joined path
/// if canonicalize fails (e.g. the configured root doesn't exist yet).
#[must_use]
pub fn resolve_src_roots(root: &Path, src: &[String]) -> Vec<PathBuf> {
    src.iter()
        .map(|entry| {
            let joined = root.join(entry);
            joined.canonicalize().unwrap_or(joined)
        })
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
        let mut files = collect_python_files(dir.path(), &[]);
        files.sort();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|p| p.extension().unwrap() == "py"));
    }

    #[test]
    fn respects_ignore_files() {
        let dir = make_tree(&["a.py", "ignored/b.py"]);
        fs::write(dir.path().join(".ignore"), "ignored/\n").expect("write .ignore");
        let files = collect_python_files(dir.path(), &[]);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("a.py"));
    }

    #[test]
    fn cli_excludes_override_pyproject() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"benchmarks/suites\"]\n",
        )
        .expect("write pyproject");
        let excludes = configured_excludes(dir.path(), &["tmp".into(), "cache".into()]);
        assert_eq!(excludes, vec!["tmp", "cache"]);
    }

    #[test]
    fn collect_python_files_respects_custom_excludes() {
        let dir = make_tree(&["a.py", "benchmarks/suites/test_bench.py"]);
        let mut files = collect_python_files(dir.path(), &["benchmarks/suites".into()]);
        files.sort();
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].line_number, Some(3));
    }

    #[test]
    fn returns_empty_for_parse_error() {
        let source = "this is not valid python @@@";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn returns_empty_for_unreadable_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nonexistent = dir.path().join("nonexistent.py");
        let items =
            parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &nonexistent).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "my_func");
    }

    #[test]
    fn cases_kwargs_form_emits_one_item_per_case() {
        let source = "@test.cases(
    zero={\"n\": 0},
    one={\"n\": 1},
    two={\"n\": 2},
)
def square(n, expected):
    expect(n * n).to_equal(expected)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 3, "expected one item per case");
        for item in &items {
            assert_eq!(item.name, "square");
        }
        let labels: Vec<_> = items
            .iter()
            .map(|i| i.case_label.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(labels, vec!["zero", "one", "two"]);
        let indices: Vec<_> = items.iter().map(|i| i.case_index).collect();
        assert_eq!(indices, vec![Some(0), Some(1), Some(2)]);
        // All cases share the same line number (the decorated function's line).
        assert_eq!(items[0].line_number, items[1].line_number);
        assert_eq!(items[1].line_number, items[2].line_number);
    }

    #[test]
    fn cases_kwargs_form_produces_unique_ids() {
        let source = "@test.cases(a={}, b={})
def fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        let ids: Vec<_> = items.iter().map(TestItem::id).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids[0].ends_with("::fn[a]"), "got {}", ids[0]);
        assert!(ids[1].ends_with("::fn[b]"), "got {}", ids[1]);
    }

    #[test]
    fn cases_typed_form_emits_one_item_per_spec() {
        let source = "@test.cases(
    test.case(\"zero\", n=0, expected=0),
    test.case(\"my test\", n=1, expected=1),
    test.case(\"2 + 3\", n=2, expected=4),
)
def square(n, expected):
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 3, "expected one item per test.case spec");
        let labels: Vec<_> = items
            .iter()
            .map(|i| i.case_label.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(labels, vec!["zero", "my test", "2 + 3"]);
    }

    #[test]
    fn cases_typed_form_rejects_non_literal_label() {
        let source = "label = \"dynamic\"
@test.cases(test.case(label, n=0))
def fn(n):
    pass
";
        let (dir, file) = write_source(source);
        let parsed = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file);
        assert!(parsed.tests.is_empty());
        assert!(
            parsed.errors.iter().any(|e| e.contains("test.case")),
            "expected an error mentioning test.case, got {:?}",
            parsed.errors
        );
    }

    #[test]
    fn cases_list_form_emits_one_item_per_entry() {
        let source = "@test.cases([
    (\"2 + 3\", {\"a\": 2, \"b\": 3, \"sum\": 5}),
    (\"-1 + 1\", {\"a\": -1, \"b\": 1, \"sum\": 0}),
])
def add(a, b, sum):
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        let labels: Vec<_> = items
            .iter()
            .map(|i| i.case_label.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(labels, vec!["2 + 3", "-1 + 1"]);
    }

    #[test]
    fn cases_inherits_describe_groups() {
        let source = "from tryke import describe
with describe(\"math\"):
    @test.cases(zero={}, one={})
    def square():
        pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        for item in &items {
            assert_eq!(item.groups, vec!["math".to_string()]);
        }
    }

    #[test]
    fn cases_composes_with_skip_modifier() {
        let source = "@test.skip(\"WIP\")
@test.cases(a={}, b={})
def fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        for item in &items {
            assert_eq!(item.skip.as_deref(), Some("WIP"));
        }
    }

    #[test]
    fn cases_per_case_skip_from_typed_form() {
        let source = r#"@test.cases(
    test.case("normal", n=1),
    test.case("broken", n=2, skip="known bug"),
)
def square(n):
    pass
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].skip, None);
        assert_eq!(items[1].skip.as_deref(), Some("known bug"));
    }

    #[test]
    fn cases_per_case_xfail_from_typed_form() {
        let source = r#"@test.cases(
    test.case("passing", n=1),
    test.case("failing", n=2, xfail="upstream issue"),
)
def check(n):
    pass
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].xfail, None);
        assert_eq!(items[1].xfail.as_deref(), Some("upstream issue"));
    }

    #[test]
    fn cases_per_case_todo_from_typed_form() {
        let source = r#"@test.cases(
    test.case("done", n=1),
    test.case("wip", n=2, todo="not implemented"),
)
def feature(n):
    pass
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].todo, None);
        assert_eq!(items[1].todo.as_deref(), Some("not implemented"));
    }

    #[test]
    fn cases_per_case_overrides_function_level() {
        let source = r#"@test.skip("default skip")
@test.cases(
    test.case("skipped_by_default", n=1),
    test.case("actually_xfail", n=2, xfail="override"),
)
def fn(n):
    pass
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        // First case inherits function-level skip.
        assert_eq!(items[0].skip.as_deref(), Some("default skip"));
        assert_eq!(items[0].xfail, None);
        // Second case has per-case xfail which takes precedence;
        // function-level skip is still inherited since xfail != skip.
        assert_eq!(items[1].skip.as_deref(), Some("default skip"));
        assert_eq!(items[1].xfail.as_deref(), Some("override"));
    }

    #[test]
    fn cases_per_case_skip_overrides_function_skip() {
        let source = r#"@test.skip("default")
@test.cases(
    test.case("inherited", n=1),
    test.case("overridden", n=2, skip="per-case reason"),
)
def fn(n):
    pass
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].skip.as_deref(), Some("default"));
        assert_eq!(items[1].skip.as_deref(), Some("per-case reason"));
    }

    #[test]
    fn cases_kwargs_form_has_no_per_case_modifiers() {
        let source = "@test.cases(a={\"skip\": \"ignored\"}, b={})
def fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        // kwargs form doesn't support per-case modifiers — "skip" inside
        // the dict is a test parameter, not a modifier.
        assert_eq!(items[0].skip, None);
        assert_eq!(items[1].skip, None);
    }

    #[test]
    fn cases_non_literal_modifier_ignored() {
        let source = r#"reason = "dynamic"
@test.cases(
    test.case("a", n=1, skip=reason),
)
def fn(n):
    pass
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        // Non-literal skip value is silently ignored at discovery time.
        assert_eq!(items[0].skip, None);
    }

    #[test]
    fn cases_plain_test_unaffected() {
        // Sanity: pre-existing @test decorator path should still produce
        // exactly one item with case_label=None.
        let source = "@test
def plain():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].case_label, None);
        assert_eq!(items[0].case_index, None);
    }

    #[test]
    fn cases_non_literal_form_emits_error() {
        let source = "@test.cases(build_cases())
def fn():
    pass
";
        let (dir, file) = write_source(source);
        let parsed = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file);
        // Non-literal decorator argument should not silently produce
        // a test — surface a diagnostic instead.
        assert!(
            parsed.tests.is_empty(),
            "expected no tests for non-literal @test.cases, got {:?}",
            parsed.tests
        );
        assert!(
            parsed.errors.iter().any(|e| e.contains("test.cases")),
            "expected an error mentioning test.cases, got {:?}",
            parsed.errors
        );
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items[0].display_name.as_deref(), Some("explicit"));
    }

    #[test]
    fn bare_test_no_display_name() {
        let source = "@test
def test_fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items[0].display_name, None);
    }

    #[test]
    fn expect_label_from_name_kwarg() {
        let source = "@test
def test_fn():
    expect(x, name=\"my label\").to_equal(1)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items[0].expected_assertions[0].label.as_deref(), Some("kw"));
    }

    #[test]
    fn expect_no_label_by_default() {
        let source = "@test
def test_fn():
    expect(x).to_equal(1)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
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
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert_eq!(imports, vec![root.join("utils.py")]);
    }

    #[test]
    fn extract_local_imports_from_absolute_submodule() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(pkg.join("__init__.py"), "").expect("write");
        fs::write(pkg.join("helpers.py"), "").expect("write");
        let source = "from pkg import helpers\n";
        let parsed = parse_module(source).expect("parse");
        let file = root.join("test_foo.py");
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert_eq!(
            imports,
            vec![
                root.join("pkg").join("__init__.py"),
                root.join("pkg").join("helpers.py")
            ]
        );
    }

    #[test]
    fn extract_local_imports_ignores_nonlocal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // stdlib / third-party (doesn't exist under root)
        let source = "import os\nimport pytest\n";
        let parsed = parse_module(source).expect("parse");
        let file = root.join("test_foo.py");
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
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
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
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
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
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
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
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
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
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
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert_eq!(imports.len(), 1);
    }

    // PEP 810: `lazy import` and `lazy from` parse as the same AST nodes as
    // their eager counterparts (with `is_lazy=true`), so the static
    // dependency graph treats them identically — editing the lazily-imported
    // file still re-runs dependents under `--changed`.
    #[test]
    fn extract_local_imports_lazy_absolute() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("utils.py"), "").expect("write");
        let source = "lazy import utils\n";
        let parsed = parse_module(source).expect("parse");
        let file = root.join("test_foo.py");
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert_eq!(imports, vec![root.join("utils.py")]);
    }

    #[test]
    fn extract_local_imports_lazy_from() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(pkg.join("__init__.py"), "").expect("write");
        fs::write(pkg.join("helpers.py"), "").expect("write");
        let source = "lazy from pkg import helpers\n";
        let parsed = parse_module(source).expect("parse");
        let file = root.join("test_foo.py");
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert_eq!(
            imports,
            vec![pkg.join("__init__.py"), pkg.join("helpers.py"),]
        );
    }

    #[test]
    fn extract_local_imports_lazy_is_not_dynamic() {
        // `lazy import` is statically visible — `has_dynamic_imports` must
        // not flag it, otherwise `--changed` falls back to always-dirty.
        let source = "lazy import utils\nlazy from pkg import sub\n";
        let parsed = parse_module(source).expect("parse");
        assert!(!has_dynamic_imports(&parsed.syntax().body));
    }

    // PEP 810 transitional mechanism: a package's `__init__.py` may declare
    // `__lazy_modules__ = ["sub"]` to expose `sub` as a lazy attribute on
    // 3.15+. We treat each entry as a static sibling-submodule dependency so
    // edits to `pkg/sub.py` mark `pkg/__init__.py` dirty.
    #[test]
    fn extract_local_imports_lazy_modules_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(pkg.join("__init__.py"), "").expect("write");
        fs::write(pkg.join("heavy.py"), "").expect("write");
        fs::write(pkg.join("other.py"), "").expect("write");
        let source = "__lazy_modules__ = [\"heavy\", \"other\"]\n";
        let parsed = parse_module(source).expect("parse");
        let file = pkg.join("__init__.py");
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert_eq!(imports, vec![pkg.join("heavy.py"), pkg.join("other.py")]);
    }

    #[test]
    fn extract_local_imports_lazy_modules_list_annotated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(pkg.join("__init__.py"), "").expect("write");
        fs::write(pkg.join("heavy.py"), "").expect("write");
        let source = "__lazy_modules__: list[str] = [\"heavy\"]\n";
        let parsed = parse_module(source).expect("parse");
        let file = pkg.join("__init__.py");
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert_eq!(imports, vec![pkg.join("heavy.py")]);
    }

    #[test]
    fn extract_local_imports_lazy_modules_resolves_subpackage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("pkg");
        let nested = pkg.join("nested");
        fs::create_dir_all(&nested).expect("mkdir");
        fs::write(pkg.join("__init__.py"), "").expect("write");
        fs::write(nested.join("__init__.py"), "").expect("write");
        let source = "__lazy_modules__ = [\"nested\"]\n";
        let parsed = parse_module(source).expect("parse");
        let file = pkg.join("__init__.py");
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert_eq!(imports, vec![nested.join("__init__.py")]);
    }

    #[test]
    fn extract_local_imports_lazy_modules_skips_non_string_entries() {
        // Mixed-type list (e.g. `[name, "other"]`) is not a valid PEP 810
        // declaration and is ignored entirely rather than partially honored.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(pkg.join("__init__.py"), "").expect("write");
        fs::write(pkg.join("other.py"), "").expect("write");
        let source = "__lazy_modules__ = [name, \"other\"]\n";
        let parsed = parse_module(source).expect("parse");
        let file = pkg.join("__init__.py");
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert!(imports.is_empty());
    }

    #[test]
    fn extract_local_imports_lazy_modules_only_when_named_correctly() {
        // A list literal bound to any other name is not a PEP 810 marker.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(pkg.join("__init__.py"), "").expect("write");
        fs::write(pkg.join("heavy.py"), "").expect("write");
        let source = "__all__ = [\"heavy\"]\n";
        let parsed = parse_module(source).expect("parse");
        let file = pkg.join("__init__.py");
        let imports =
            extract_local_imports(root, &[root.to_path_buf()], &file, &parsed.syntax().body);
        assert!(imports.is_empty());
    }

    #[test]
    fn extracts_assertion_with_fatal() {
        let source = "@test
def test_fn():
    expect(x).to_equal(1).fatal()
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
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
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items[0].expected_assertions.len(), 1);
        let a = &items[0].expected_assertions[0];
        assert_eq!(a.subject, "x");
        assert_eq!(a.matcher, "to_be_none");
        assert!(a.negated);
    }

    // --- test.skip / test.todo / test.xfail decorator recognition ---

    #[test]
    fn recognizes_test_skip_bare() {
        let source = "@test.skip\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].skip.as_deref(), Some(""));
    }

    #[test]
    fn recognizes_test_skip_with_reason() {
        let source = "@test.skip(\"broken\")\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].skip.as_deref(), Some("broken"));
    }

    #[test]
    fn recognizes_test_skip_reason_kwarg() {
        let source = "@test.skip(reason=\"broken\")\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items[0].skip.as_deref(), Some("broken"));
    }

    #[test]
    fn recognizes_test_todo_bare() {
        let source = "@test.todo\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].todo.as_deref(), Some(""));
    }

    #[test]
    fn recognizes_test_todo_with_description() {
        let source = "@test.todo(\"need caching\")\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items[0].todo.as_deref(), Some("need caching"));
    }

    #[test]
    fn recognizes_test_xfail_bare() {
        let source = "@test.xfail\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].xfail.as_deref(), Some(""));
    }

    #[test]
    fn recognizes_test_xfail_with_reason() {
        let source = "@test.xfail(\"upstream bug\")\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items[0].xfail.as_deref(), Some("upstream bug"));
    }

    #[test]
    fn recognizes_qualified_test_skip() {
        let source = "import tryke\n@tryke.test.skip(\"broken\")\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].skip.as_deref(), Some("broken"));
    }

    #[test]
    fn recognizes_test_skip_if() {
        let source = "@test.skip_if(True, reason=\"always\")\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        // skip_if cannot be resolved statically
        assert!(items[0].skip.is_none());
    }

    #[test]
    fn extracts_tags_from_test_decorator() {
        let source = "@test(tags=[\"slow\", \"network\"])\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].tags, vec!["slow", "network"]);
    }

    #[test]
    fn extracts_tags_from_skip_decorator() {
        let source = "@test.skip(\"broken\", tags=[\"admin\"])\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].tags, vec!["admin"]);
        assert_eq!(items[0].skip.as_deref(), Some("broken"));
    }

    #[test]
    fn no_tags_by_default() {
        let source = "@test\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert!(items[0].tags.is_empty());
    }

    #[test]
    fn plain_test_has_no_modifiers() {
        let source = "@test\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert!(items[0].skip.is_none());
        assert!(items[0].todo.is_none());
        assert!(items[0].xfail.is_none());
    }

    // --- describe block tests ---

    #[test]
    fn discovers_tests_in_describe_block() {
        let source = "\
with describe(\"Math\"):
    @test
    def test_add():
        expect(1 + 1).to_equal(2)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "test_add");
        assert_eq!(items[0].groups, vec!["Math"]);
    }

    #[test]
    fn discovers_tests_in_nested_describe() {
        let source = "\
with describe(\"Math\"):
    with describe(\"addition\"):
        @test
        def test_add():
            pass
    with describe(\"subtraction\"):
        @test
        def test_sub():
            pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "test_add");
        assert_eq!(items[0].groups, vec!["Math", "addition"]);
        assert_eq!(items[1].name, "test_sub");
        assert_eq!(items[1].groups, vec!["Math", "subtraction"]);
    }

    #[test]
    fn top_level_tests_have_empty_groups() {
        let source = "@test\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert!(items[0].groups.is_empty());
    }

    #[test]
    fn mixed_describe_and_top_level() {
        let source = "\
with describe(\"Group\"):
    @test
    def test_grouped():
        pass

@test
def test_standalone():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].groups, vec!["Group"]);
        assert!(items[1].groups.is_empty());
    }

    #[test]
    fn describe_with_tryke_qualified() {
        let source = "\
with tryke.describe(\"Suite\"):
    @test
    def test_fn():
        pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].groups, vec!["Suite"]);
    }

    #[test]
    fn describe_preserves_test_metadata() {
        let source = "\
with describe(\"Group\"):
    @test.skip(\"broken\")
    def test_fn():
        expect(1).to_equal(2)
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].groups, vec!["Group"]);
        assert_eq!(items[0].skip.as_deref(), Some("broken"));
        assert_eq!(items[0].expected_assertions.len(), 1);
    }

    // --- has_dynamic_imports tests ---

    fn parse_body(source: &str) -> Vec<Stmt> {
        parse_module(source).expect("parse").into_syntax().body
    }

    #[test]
    fn detects_importlib_import_module() {
        let body = parse_body("import importlib\nmod = importlib.import_module('foo')\n");
        assert!(has_dynamic_imports(&body));
    }

    #[test]
    fn detects_dunder_import() {
        let body = parse_body("mod = __import__('foo')\n");
        assert!(has_dynamic_imports(&body));
    }

    #[test]
    fn no_dynamic_imports_in_static_code() {
        let body = parse_body("import os\nfrom pathlib import Path\n");
        assert!(!has_dynamic_imports(&body));
    }

    #[test]
    fn detects_dynamic_import_inside_function() {
        let body = parse_body("def load():\n    importlib.import_module('bar')\n");
        assert!(has_dynamic_imports(&body));
    }

    #[test]
    fn detects_dynamic_import_inside_if() {
        let body = parse_body("if True:\n    __import__('baz')\n");
        assert!(has_dynamic_imports(&body));
    }

    #[test]
    fn detects_dynamic_import_inside_try() {
        let body = parse_body("try:\n    importlib.import_module('x')\nexcept:\n    pass\n");
        assert!(has_dynamic_imports(&body));
    }

    #[test]
    fn discover_from_returns_tests_in_line_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let source = "\
@test
def test_third():
    pass

@test
def test_first():
    pass

@test
def test_second():
    pass
";
        fs::write(dir.path().join("test_order.py"), source).expect("write test file");
        let items = discover_from(dir.path());
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].name, "test_third");
        assert_eq!(items[1].name, "test_first");
        assert_eq!(items[2].name, "test_second");
        // Line numbers are monotonically increasing
        for pair in items.windows(2) {
            assert!(pair[0].line_number < pair[1].line_number);
        }
    }

    #[test]
    fn discovers_function_doctest() {
        let source = r#"
def add(a, b):
    """Add two numbers.

    >>> add(1, 2)
    3
    """
    return a + b
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "add");
        assert_eq!(items[0].doctest_object, Some("add".to_string()));
        assert_eq!(items[0].display_name, Some("doctest: add".to_string()));
    }

    #[test]
    fn discovers_class_and_method_doctests() {
        let source = r#"
class Calc:
    """A calculator.

    >>> c = Calc()
    >>> c.value
    0
    """

    def __init__(self):
        self.value = 0

    def add(self, n):
        """Add n.

        >>> c = Calc()
        >>> c.add(5)
        >>> c.value
        5
        """
        self.value += n
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].doctest_object, Some("Calc".to_string()));
        assert_eq!(items[1].doctest_object, Some("Calc.add".to_string()));
    }

    #[test]
    fn discovers_module_level_doctest() {
        let source = r#"
"""Module with doctest.

>>> 1 + 1
2
"""

def helper():
    pass
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "__module__");
        assert_eq!(items[0].doctest_object, Some(String::new()));
    }

    #[test]
    fn no_doctests_without_chevrons() {
        let source = r#"
def foo():
    """Just a plain docstring."""
    pass
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert!(items.is_empty());
    }

    #[test]
    fn doctests_and_decorated_tests_coexist() {
        let source = r#"
from tryke import test, expect

def add(a, b):
    """Add two numbers.

    >>> add(1, 2)
    3
    """
    return a + b

@test
def test_add():
    expect(add(1, 2)).to_equal(3)
"#;
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        // Decorated test comes first (collect_tests_from_body runs first)
        assert_eq!(items[0].name, "test_add");
        assert!(items[0].doctest_object.is_none());
        // Doctest comes second
        assert_eq!(items[1].name, "add");
        assert_eq!(items[1].doctest_object, Some("add".to_string()));
    }

    // ---- Fixture discovery tests ----

    #[test]
    fn discovers_bare_fixture_at_module_level() {
        let source = "@fixture\ndef setup():\n    pass\n\n@test\ndef test_fn():\n    pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks.len(), 1);
        assert_eq!(parsed.hooks[0].name, "setup");
        assert_eq!(parsed.hooks[0].per, FixturePer::Test);
        assert!(parsed.hooks[0].groups.is_empty());
    }

    #[test]
    fn discovers_scope_fixture_via_kwarg() {
        let source = "@fixture(per=\"scope\")\ndef db(): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks.len(), 1);
        assert_eq!(parsed.hooks[0].per, FixturePer::Scope);
    }

    #[test]
    fn discovers_test_fixture_via_explicit_kwarg() {
        let source = "@fixture(per=\"test\")\ndef setup(): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks[0].per, FixturePer::Test);
    }

    #[test]
    fn discovers_call_form_fixture_without_kwargs() {
        let source = "@fixture()\ndef setup(): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks.len(), 1);
        assert_eq!(parsed.hooks[0].per, FixturePer::Test);
    }

    #[test]
    fn discovers_fixture_inside_describe_block() {
        let source = "with describe(\"users\"):\n    @fixture\n    def seed(): pass\n    @test\n    def test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks.len(), 1);
        assert_eq!(parsed.hooks[0].name, "seed");
        assert_eq!(parsed.hooks[0].groups, vec!["users"]);
    }

    #[test]
    fn discovers_qualified_tryke_fixture() {
        let source = "import tryke\n@tryke.fixture(per=\"scope\")\ndef db(): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks.len(), 1);
        assert_eq!(parsed.hooks[0].per, FixturePer::Scope);
    }

    #[test]
    fn extracts_depends_from_fixture_params() {
        let source = "\
@fixture(per=\"scope\")\ndef db(): pass\n\
@fixture\ndef table(conn=Depends(db)): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks.len(), 2);
        let table_hook = parsed
            .hooks
            .iter()
            .find(|h| h.name == "table")
            .expect("fixture 'table' must exist in parsed output");
        assert_eq!(table_hook.depends_on, vec!["db"]);
        assert!(
            parsed.errors.is_empty(),
            "no errors expected: {:?}",
            parsed.errors
        );
    }

    #[test]
    fn extracts_multiple_depends() {
        let source = "\
@fixture(per=\"scope\")\ndef db(): pass\n\
@fixture(per=\"scope\")\ndef cache(): pass\n\
@fixture\ndef svc(d=Depends(db), c=Depends(cache)): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        let svc = parsed
            .hooks
            .iter()
            .find(|h| h.name == "svc")
            .expect("fixture 'svc' must exist in parsed output");
        assert_eq!(svc.depends_on, vec!["db", "cache"]);
        assert!(
            parsed.errors.is_empty(),
            "no errors expected: {:?}",
            parsed.errors
        );
    }

    #[test]
    fn fixture_without_depends_has_empty_depends_on() {
        let source = "@fixture\ndef setup(): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert!(parsed.hooks[0].depends_on.is_empty());
        assert!(parsed.errors.is_empty());
    }

    #[test]
    fn depends_with_attribute_arg_is_a_discovery_error() {
        let source = "\
@fixture\ndef svc(x=Depends(mod.fn)): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks.len(), 1);
        assert!(parsed.hooks[0].depends_on.is_empty());
        assert_eq!(
            parsed.errors.len(),
            1,
            "expected one error, got {:?}",
            parsed.errors
        );
        let err = &parsed.errors[0];
        assert!(err.contains("Depends(attribute)"), "error: {err}");
        assert!(err.contains("svc"), "error should name the fixture: {err}");
        assert!(
            err.contains(":2:"),
            "error should include line 2 (the def): {err}"
        );
    }

    #[test]
    fn depends_with_no_args_is_a_discovery_error() {
        let source = "\
@fixture\ndef svc(x=Depends()): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks.len(), 1);
        assert!(parsed.hooks[0].depends_on.is_empty());
        assert_eq!(parsed.errors.len(), 1);
        let err = &parsed.errors[0];
        assert!(
            err.contains("requires exactly one positional argument"),
            "error: {err}"
        );
    }

    #[test]
    fn depends_with_call_arg_is_a_discovery_error() {
        let source = "\
@fixture\ndef svc(x=Depends(factory())): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks.len(), 1);
        assert!(parsed.hooks[0].depends_on.is_empty());
        assert_eq!(parsed.errors.len(), 1);
        assert!(parsed.errors[0].contains("Depends(call)"));
    }

    #[test]
    fn valid_and_invalid_depends_are_reported_independently() {
        let source = "\
@fixture(per=\"scope\")\ndef db(): pass\n\
@fixture\ndef svc(a=Depends(db), b=Depends(unsupported.thing)): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        let svc = parsed
            .hooks
            .iter()
            .find(|h| h.name == "svc")
            .expect("svc fixture");
        assert_eq!(svc.depends_on, vec!["db"]);
        assert_eq!(parsed.errors.len(), 1);
        assert!(parsed.errors[0].contains("Depends(attribute)"));
    }

    #[test]
    fn fixtures_and_tests_coexist() {
        let source = "\
@fixture\ndef setup(): pass\n\
@test\ndef test_fn(): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.tests.len(), 1);
        assert_eq!(parsed.hooks.len(), 1);
    }

    #[test]
    fn locally_defined_fixture_is_not_a_fixture() {
        let source = "\
def fixture(fn):\n    return fn\n\
@fixture\ndef setup(): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert!(parsed.hooks.is_empty());
    }

    #[test]
    fn fixture_has_line_number() {
        let source = "\n\n@fixture\ndef setup(): pass\n";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.hooks[0].line_number, Some(3));
    }

    // ------------------------------------------------------------------
    // __TRYKE_TESTING__ guard tests
    // ------------------------------------------------------------------

    #[test]
    fn tests_inside_testing_guard_are_discovered() {
        let source = "\
from tryke_guard import __TRYKE_TESTING__

if __TRYKE_TESTING__:
    from tryke import test
    @test
    def guarded():
        pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        let names: Vec<&str> = parsed.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["guarded"]);
    }

    #[test]
    fn attribute_form_guard_is_recognized() {
        let source = "\
import tryke_guard

if tryke_guard.__TRYKE_TESTING__:
    from tryke import test
    @test
    def guarded():
        pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        let names: Vec<&str> = parsed.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["guarded"]);
    }

    #[test]
    fn plain_if_block_does_not_descend() {
        // Regression: tests inside a non-guard `if` should NOT be collected,
        // preserving today's behavior.
        let source = "\
if CONFIG_FLAG:
    @test
    def should_not_be_found():
        pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert!(parsed.tests.is_empty());
    }

    #[test]
    fn negated_guard_does_not_descend() {
        let source = "\
if not __TRYKE_TESTING__:
    @test
    def prod_only():
        pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert!(parsed.tests.is_empty());
    }

    #[test]
    fn guard_with_else_skips_tests_and_emits_warning() {
        let source = "\
if __TRYKE_TESTING__:
    @test
    def dropped():
        pass
else:
    pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        // Discovery must NOT pick up tests under a guard with else.
        assert!(parsed.tests.is_empty());
        // But it MUST record the line so a warning is surfaced.
        assert_eq!(parsed.testing_guard_else_lines, vec![1]);
    }

    #[test]
    fn guard_with_elif_emits_warning() {
        let source = "\
if __TRYKE_TESTING__:
    @test
    def dropped():
        pass
elif OTHER_FLAG:
    pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert!(parsed.tests.is_empty());
        assert_eq!(parsed.testing_guard_else_lines, vec![1]);
    }

    #[test]
    fn guard_without_else_emits_no_warning() {
        let source = "\
if __TRYKE_TESTING__:
    @test
    def ok():
        pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert!(parsed.testing_guard_else_lines.is_empty());
    }

    #[test]
    fn non_guard_if_with_else_emits_no_warning() {
        // Regression: plain if/else without the guard condition must not
        // trigger the testing-guard warning.
        let source = "\
if CONFIG:
    pass
else:
    pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert!(parsed.testing_guard_else_lines.is_empty());
    }

    #[test]
    fn imports_inside_guard_resolve_test_decorator() {
        // Pin the invariant: is_locally_defined only scans function/class/
        // assign statements, so a nested `from tryke import test` inside
        // `if __TRYKE_TESTING__:` does NOT shadow the bare `test` decorator.
        let source = "\
if __TRYKE_TESTING__:
    from tryke import test
    @test
    def guarded():
        pass
    @test.skip(\"not yet\")
    def skipped():
        pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        let names: Vec<&str> = parsed.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"guarded"));
        assert!(names.contains(&"skipped"));
    }

    #[test]
    fn fixtures_inside_testing_guard_are_discovered() {
        let source = "\
if __TRYKE_TESTING__:
    from tryke import fixture
    @fixture
    def db():
        yield \"conn\"
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        let names: Vec<&str> = parsed.hooks.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["db"]);
    }

    #[test]
    fn doctests_inside_testing_guard_are_discovered() {
        let source = "\
if __TRYKE_TESTING__:
    def add(a, b):
        \"\"\"Add two numbers.

        >>> add(1, 2)
        3
        \"\"\"
        return a + b
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        let doctests: Vec<&str> = parsed
            .tests
            .iter()
            .filter(|t| t.doctest_object.is_some())
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(doctests, vec!["add"]);
    }

    #[test]
    fn imports_inside_guard_are_in_graph() {
        // Imports inside `if __TRYKE_TESTING__:` must contribute to the
        // static import graph so --changed precision is preserved.
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        fs::write(dir.path().join("helpers.py"), "VALUE = 1\n").expect("write helpers.py");
        let user_src = "\
from tryke_guard import __TRYKE_TESTING__

if __TRYKE_TESTING__:
    from tryke import test, expect
    import helpers

    @test
    def uses_helpers():
        expect(helpers.VALUE).to_equal(1)
";
        fs::write(dir.path().join("user.py"), user_src).expect("write user.py");

        let mut discoverer = crate::Discoverer::new(dir.path());
        discoverer.rediscover();
        // Changing helpers.py must re-select user.py because the import
        // graph now follows the guard-nested import of `helpers`.
        let changed = vec![dir.path().join("helpers.py")];
        let tests = discoverer.tests_for_changed(&changed);
        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"uses_helpers"),
            "expected uses_helpers in affected tests, got: {names:?}"
        );
    }

    #[test]
    fn dynamic_import_inside_guard_does_not_mark_always_dirty() {
        // An `importlib.import_module(...)` call inside `if __TRYKE_TESTING__:`
        // is unreachable in production — it must not flag the file as
        // always-dirty for `--changed`.
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let guarded_dyn = "\
from tryke_guard import __TRYKE_TESTING__

if __TRYKE_TESTING__:
    import importlib
    from tryke import test

    @test
    def ok():
        mod = importlib.import_module('os')
";
        fs::write(dir.path().join("test_guarded_dyn.py"), guarded_dyn).expect("write");

        let mut discoverer = crate::Discoverer::new(dir.path());
        discoverer.rediscover();
        let files = discoverer.dynamic_import_files();
        let names: Vec<&str> = files
            .iter()
            .filter_map(|p| p.file_name())
            .filter_map(|n| n.to_str())
            .collect();
        assert!(
            !names.contains(&"test_guarded_dyn.py"),
            "guarded dynamic import must NOT mark file always-dirty, got: {names:?}"
        );
    }

    #[test]
    fn unguarded_dynamic_import_still_marks_always_dirty() {
        // Regression: the guard-exemption in stmt_has_dynamic_import must
        // not suppress legitimate always-dirty flags for non-guarded
        // dynamic imports.
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let raw_dyn = "\
import importlib
mod = importlib.import_module('os')
from tryke import test
@test
def t(): pass
";
        fs::write(dir.path().join("test_raw_dyn.py"), raw_dyn).expect("write");

        let mut discoverer = crate::Discoverer::new(dir.path());
        discoverer.rediscover();
        let files = discoverer.dynamic_import_files();
        let names: Vec<&str> = files
            .iter()
            .filter_map(|p| p.file_name())
            .filter_map(|n| n.to_str())
            .collect();
        assert!(
            names.contains(&"test_raw_dyn.py"),
            "unguarded dynamic import MUST mark file always-dirty, got: {names:?}"
        );
    }

    #[test]
    fn guard_inside_describe_inherits_group() {
        // Tests inside `with describe(...): if __TRYKE_TESTING__:` should
        // carry the describe group through.
        let source = "\
from tryke import describe

with describe(\"math\"):
    if __TRYKE_TESTING__:
        from tryke import test
        @test
        def addition():
            pass
";
        let (dir, file) = write_source(source);
        let parsed =
            parse_tests_from_source(dir.path(), &[dir.path().to_path_buf()], &file, source);
        assert_eq!(parsed.tests.len(), 1);
        assert_eq!(parsed.tests[0].name, "addition");
        assert_eq!(parsed.tests[0].groups, vec!["math".to_string()]);
    }

    #[test]
    fn recognizes_tryke_module_alias() {
        let source = "\
import tryke as t
with t.describe(\"Channel\"):
    @t.test
    def test_basic():
        pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "test_basic");
        assert_eq!(items[0].groups, vec!["Channel".to_string()]);
    }

    #[test]
    fn recognizes_tryke_fixture_alias() {
        let source = "\
import tryke as t
@t.fixture
def db():
    pass
";
        let (dir, file) = write_source(source);
        let parsed = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file);
        assert_eq!(parsed.hooks.len(), 1);
        assert_eq!(parsed.hooks[0].name, "db");
    }

    #[test]
    fn recognizes_tryke_test_cases_alias() {
        let source = "\
import tryke as tk
@tk.test.cases(a={}, b={})
def fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        let labels: Vec<_> = items
            .iter()
            .map(|i| i.case_label.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(labels, vec!["a", "b"]);
    }

    #[test]
    fn recognizes_tryke_test_case_typed_alias() {
        let source = "\
import tryke as t
@t.test.cases(
    t.test.case(\"zero\", n=0),
    t.test.case(\"one\", n=1),
)
def fn(n):
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 2);
        let labels: Vec<_> = items
            .iter()
            .map(|i| i.case_label.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(labels, vec!["zero", "one"]);
    }

    #[test]
    fn recognizes_symbol_alias_bare_describe() {
        let source = "\
from tryke import describe as d, test as tst
with d(\"Group\"):
    @tst
    def fn():
        pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "fn");
        assert_eq!(items[0].groups, vec!["Group".to_string()]);
    }

    #[test]
    fn recognizes_symbol_alias_test_skip() {
        let source = "\
from tryke import test as tst
@tst.skip(\"broken\")
def fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].skip.as_deref(), Some("broken"));
    }

    #[test]
    fn recognizes_alias_inside_testing_guard() {
        // Screenshot scenario: `import tryke as t` sits inside the
        // `if __TRYKE_TESTING__:` guard. Discovery must still see it.
        let source = "\
from tryke_guard import __TRYKE_TESTING__

if __TRYKE_TESTING__:
    import tryke as t

    with t.describe(name=\"Channel\"):
        @t.test
        def test_basic() -> None:
            pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "test_basic");
        assert_eq!(items[0].groups, vec!["Channel".to_string()]);
    }

    #[test]
    fn local_def_shadows_imported_alias() {
        // A local `def tst` wins over `from tryke import test as tst`,
        // matching Python scoping.
        let source = "\
from tryke import test as tst

def tst(fn):
    return fn

@tst
def fn():
    pass
";
        let (dir, file) = write_source(source);
        let items = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file).tests;
        assert!(items.is_empty(), "expected shadowed alias to not match");
    }

    #[test]
    fn recognizes_aliased_depends() {
        let source = "\
from tryke import fixture, Depends as Dep
@fixture
def parent():
    pass

@fixture
def child(p=Dep(parent)):
    pass
";
        let (dir, file) = write_source(source);
        let parsed = parse_tests_from_file(dir.path(), &[dir.path().to_path_buf()], &file);
        let child = parsed
            .hooks
            .iter()
            .find(|h| h.name == "child")
            .expect("child fixture not found");
        assert_eq!(child.depends_on, vec!["parent".to_string()]);
    }
}
