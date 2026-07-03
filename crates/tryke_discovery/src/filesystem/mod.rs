use std::path::{Path, PathBuf};
use std::{env, fs};

use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use log::trace;
use rayon::prelude::*;
use tryke_types::{ParsedFile, TestItem};

pub(crate) mod cache;
pub(crate) mod db;
mod discoverer;
pub(crate) mod import_graph;

pub use cache::{CleanCacheReport, clean_project_cache};
pub use discoverer::Discoverer;

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

/// Build the full ignore matcher used to decide whether an incoming
/// path (from the FS watcher or a `did_change` RPC) should reach
/// discovery. Layers `.gitignore`, `.ignore`, and the project's
/// `[tool.tryke] exclude` list — same composition the FS watcher uses,
/// extracted so the `did_change` path stays consistent.
///
/// Returns an empty matcher (matches nothing) on build failure rather
/// than propagating an error, matching the watcher's existing
/// behaviour: degraded into "let everything through" is preferable to
/// blocking discovery entirely.
#[must_use]
pub fn build_change_set_ignore(root: &Path, excludes: &[String]) -> Gitignore {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut builder = GitignoreBuilder::new(&canonical);
    let _ = builder.add(canonical.join(".gitignore"));
    let _ = builder.add(canonical.join(".ignore"));
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

pub(crate) fn discover_file_from_ast(
    root: &Path,
    src_roots: &[PathBuf],
    file: &Path,
    parsed: &db::ParsedAst,
) -> tryke_types::DiscoveredFile {
    let Some(module) = parsed.syntax() else {
        trace!("parse error in {}", file.display());
        return tryke_types::DiscoveredFile::default();
    };
    let result = crate::source::discover_file_from_body(
        root,
        src_roots,
        file,
        &module.body,
        parsed.source(),
    );
    for err in &result.parsed.errors {
        log::error!("tryke discovery: {err}");
    }
    result
}

fn parse_tests_from_file(root: &Path, src_roots: &[PathBuf], file: &Path) -> ParsedFile {
    let Ok(source) = fs::read_to_string(file) else {
        return ParsedFile::default();
    };
    crate::source::parse_tests_from_source(root, src_roots, file, &source)
}

#[must_use]
pub fn discover_from(start: &Path) -> Vec<TestItem> {
    let config = tryke_config::load_effective_config(start);
    let root = find_project_root(start).unwrap_or_else(|| start.to_path_buf());
    let src_roots = config.discovery.src_roots(&root);
    discover_from_with_options(&root, &config.discovery.exclude, &src_roots)
}

#[must_use]
pub fn discover_from_with_excludes(start: &Path, excludes: &[String]) -> Vec<TestItem> {
    let config = tryke_config::load_effective_config(start);
    let root = find_project_root(start).unwrap_or_else(|| start.to_path_buf());
    let src_roots = config.discovery.src_roots(&root);
    discover_from_with_options(&root, excludes, &src_roots)
}

#[must_use]
pub fn discover_from_with_options(
    root: &Path,
    excludes: &[String],
    src_roots: &[PathBuf],
) -> Vec<TestItem> {
    let mut files = collect_python_files(root, excludes);
    files.sort();
    let parsed: Vec<ParsedFile> = files
        .par_iter()
        .map(|f| parse_tests_from_file(root, src_roots, f))
        .collect();
    let mut tests: Vec<TestItem> = parsed.into_iter().flat_map(|p| p.tests).collect();
    tests.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.line_number.cmp(&b.line_number))
    });
    tests
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

    fn make_discoverer(root: &Path) -> Discoverer {
        let src_roots = tryke_config::DiscoveryConfig::default().src_roots(root);
        Discoverer::new(root, src_roots, &[], None)
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
            "[tool.tryke]\nexclude = [\"generated/suites\"]\n",
        )
        .expect("write pyproject");
        let excludes = configured_excludes(dir.path(), &["tmp".into(), "cache".into()]);
        assert_eq!(excludes, vec!["tmp", "cache"]);
    }

    #[test]
    fn collect_python_files_respects_custom_excludes() {
        let dir = make_tree(&["a.py", "generated/suites/test_generated.py"]);
        let mut files = collect_python_files(dir.path(), &["generated/suites".into()]);
        files.sort();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("a.py"));
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
        for pair in items.windows(2) {
            assert!(pair[0].line_number < pair[1].line_number);
        }
    }

    #[test]
    fn imports_inside_guard_are_in_graph() {
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

        let mut discoverer = make_discoverer(dir.path());
        discoverer.rediscover();
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

        let mut discoverer = make_discoverer(dir.path());
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

        let mut discoverer = make_discoverer(dir.path());
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
}
