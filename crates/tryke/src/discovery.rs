use std::path::{Path, PathBuf};

use log::{debug, warn};
use tryke_config::load_effective_config;
use tryke_discovery::Discoverer;
use tryke_types::filter::PathSpec;
use tryke_types::{DiscoveryWarning, DiscoveryWarningKind, HookItem};

use crate::git::resolve_changed_files;

pub struct DiscoverySelection {
    pub tests: Vec<tryke_types::TestItem>,
    /// Lifecycle hooks discovered alongside tests.
    pub hooks: Vec<HookItem>,
    pub changed_files: Option<usize>,
    /// In changed-first mode, how many tests at the front are "changed" tests.
    pub changed_prefix_len: Option<usize>,
    /// Files where dynamic imports were detected; these will always re-run with --changed.
    pub warnings: Vec<DiscoveryWarning>,
}

fn dynamic_import_warnings(discoverer: &Discoverer) -> Vec<DiscoveryWarning> {
    discoverer
        .dynamic_import_files()
        .into_iter()
        .map(|path| {
            let message = format!(
                "{} — dynamic imports found; will always re-run with --changed",
                path.display()
            );
            DiscoveryWarning {
                file_path: path,
                kind: DiscoveryWarningKind::DynamicImports,
                message,
            }
        })
        .collect()
}

fn testing_guard_else_warnings(discoverer: &Discoverer) -> Vec<DiscoveryWarning> {
    discoverer
        .testing_guard_else_locations()
        .into_iter()
        .map(|(path, line)| {
            let message = format!(
                "{}:{line} — `if __TRYKE_TESTING__:` has elif/else; tests inside will NOT be \
                 discovered. Move production fallback code above or below the guard.",
                path.display()
            );
            DiscoveryWarning {
                file_path: path,
                kind: DiscoveryWarningKind::TestingGuardHasElseBranch,
                message,
            }
        })
        .collect()
}

fn all_discovery_warnings(discoverer: &Discoverer) -> Vec<DiscoveryWarning> {
    let mut warnings = dynamic_import_warnings(discoverer);
    warnings.extend(testing_guard_else_warnings(discoverer));
    warnings
}

pub fn resolved_excludes(
    root: &Path,
    cli_excludes: &[String],
    cli_includes: &[String],
) -> Vec<String> {
    if !cli_excludes.is_empty() {
        return cli_excludes.to_vec();
    }
    let includes = cli_includes
        .iter()
        .collect::<std::collections::HashSet<_>>();
    load_effective_config(root)
        .discovery
        .exclude
        .into_iter()
        .filter(|exclude| !includes.contains(exclude))
        .collect()
}

/// Discover tests, optionally restricting to changed files.
pub fn discover_tests(
    root: &Path,
    changed: bool,
    base_branch: Option<&str>,
    excludes: &[String],
) -> DiscoverySelection {
    let mut discoverer = Discoverer::new_with_excludes(root, excludes);
    discoverer.rediscover();
    let warnings = all_discovery_warnings(&discoverer);
    let hooks = discoverer.hooks();

    if changed {
        match resolve_changed_files(root, base_branch) {
            Some(changed_files) if !changed_files.is_empty() => {
                debug!("--changed: {} git-changed files", changed_files.len());
                DiscoverySelection {
                    tests: discoverer.tests_for_changed(&changed_files),
                    hooks,
                    changed_files: Some(changed_files.len()),
                    changed_prefix_len: None,
                    warnings,
                }
            }
            Some(_) => {
                debug!("--changed: no changed files found via git, selecting nothing");
                DiscoverySelection {
                    tests: Vec::new(),
                    hooks,
                    changed_files: Some(0),
                    changed_prefix_len: None,
                    warnings,
                }
            }
            None => {
                warn!("--changed: git unavailable or failed, running all tests");
                DiscoverySelection {
                    tests: discoverer.tests(),
                    hooks,
                    changed_files: None,
                    changed_prefix_len: None,
                    warnings,
                }
            }
        }
    } else {
        DiscoverySelection {
            tests: discoverer.tests(),
            hooks,
            changed_files: None,
            changed_prefix_len: None,
            warnings,
        }
    }
}

/// Discover tests restricted to the given path specs. Skips the full
/// project walk and the import-graph build, since path-restricted runs
/// don't drive change-based selection. Falls back to `discover_tests`
/// if any spec resolves to a nonexistent file or escapes the project
/// root — the existing post-filter (`TestFilter::apply`) still runs in
/// `main` and handles suffix-match semantics in that case.
pub fn discover_tests_for_paths(
    root: &Path,
    path_specs: &[PathSpec],
    excludes: &[String],
) -> DiscoverySelection {
    let walk_roots = match resolve_walk_roots(root, path_specs) {
        Some(roots) => roots,
        None => {
            debug!("discover_tests_for_paths: falling back to full discovery");
            return discover_tests(root, false, None, excludes);
        }
    };

    let mut discoverer = Discoverer::new_with_excludes(root, excludes);
    let tests = discoverer.rediscover_restricted(&walk_roots);
    let warnings = all_discovery_warnings(&discoverer);
    let hooks = discoverer.hooks();
    DiscoverySelection {
        tests,
        hooks,
        changed_files: None,
        changed_prefix_len: None,
        warnings,
    }
}

/// Translate `PathSpec`s into a deduplicated list of filesystem walk
/// roots. Returns `None` if any spec resolves to a missing path or
/// escapes `root`, signalling the caller to fall back to the full walk.
fn resolve_walk_roots(root: &Path, path_specs: &[PathSpec]) -> Option<Vec<PathBuf>> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut walk_roots: Vec<PathBuf> = Vec::with_capacity(path_specs.len());
    for spec in path_specs {
        let raw = match spec {
            PathSpec::File(p) | PathSpec::FileLine(p, _) => p.clone(),
        };
        let abs = if raw.is_absolute() {
            raw
        } else {
            root.join(&raw)
        };
        let Ok(resolved) = abs.canonicalize() else {
            debug!(
                "discover_tests_for_paths: {} does not exist on disk",
                abs.display()
            );
            return None;
        };
        if !resolved.starts_with(&canonical_root) {
            warn!(
                "discover_tests_for_paths: {} is outside project root {}, falling back",
                resolved.display(),
                canonical_root.display()
            );
            return None;
        }
        // Both files and directories are walked via `WalkBuilder` inside
        // `collect_python_files_restricted`; non-`.py` files yield no
        // tests (the extension filter drops them) which mirrors the
        // existing post-filter semantics.
        walk_roots.push(resolved);
    }

    // Dedupe by ancestry: a file under a kept directory is redundant.
    walk_roots.sort_by_key(|p| p.components().count());
    let mut deduped: Vec<PathBuf> = Vec::new();
    for r in walk_roots {
        if !deduped.iter().any(|kept| r.starts_with(kept)) {
            deduped.push(r);
        }
    }
    Some(deduped)
}

/// Discover all tests but place changed tests first in the returned list.
pub fn discover_tests_changed_first(
    root: &Path,
    base_branch: Option<&str>,
    excludes: &[String],
) -> DiscoverySelection {
    let mut discoverer = Discoverer::new_with_excludes(root, excludes);
    discoverer.rediscover();
    let warnings = all_discovery_warnings(&discoverer);
    let hooks = discoverer.hooks();
    let changed_files = resolve_changed_files(root, base_branch);
    let all_tests = discoverer.tests();
    match changed_files {
        Some(cf) if !cf.is_empty() => {
            let changed_tests = discoverer.tests_for_changed(&cf);
            let changed_ids: std::collections::HashSet<String> =
                changed_tests.iter().map(|t| t.id()).collect();
            let (first, rest): (Vec<_>, Vec<_>) = all_tests
                .into_iter()
                .partition(|t| changed_ids.contains(&t.id()));
            let changed_prefix_len = first.len();
            let mut tests = first;
            tests.extend(rest);
            DiscoverySelection {
                tests,
                hooks,
                changed_files: Some(cf.len()),
                changed_prefix_len: Some(changed_prefix_len),
                warnings,
            }
        }
        Some(_) => {
            warn!("--changed-first: no changed files found, running all tests in default order");
            DiscoverySelection {
                tests: all_tests,
                hooks,
                changed_files: None,
                changed_prefix_len: None,
                warnings,
            }
        }
        None => {
            warn!("--changed-first: git unavailable, running all tests in default order");
            DiscoverySelection {
                tests: all_tests,
                hooks,
                changed_files: None,
                changed_prefix_len: None,
                warnings,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_helpers::*;

    #[test]
    fn resolved_excludes_reads_pyproject_when_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"benchmarks/suites\"]\n",
        )
        .expect("write pyproject.toml");

        let excludes = resolved_excludes(dir.path(), &[], &[]);
        assert_eq!(excludes, vec!["benchmarks/suites"]);
    }

    #[test]
    fn resolved_excludes_removes_included_config_excludes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"benchmarks/suites\", \"generated\"]\n",
        )
        .expect("write pyproject.toml");

        let excludes = resolved_excludes(dir.path(), &[], &["benchmarks/suites".into()]);
        assert_eq!(excludes, vec!["generated"]);
    }

    #[test]
    fn resolved_excludes_prefers_cli_excludes_over_includes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nexclude = [\"benchmarks/suites\"]\n",
        )
        .expect("write pyproject.toml");

        let excludes = resolved_excludes(
            dir.path(),
            &["tmp".into(), "cache".into()],
            &["benchmarks/suites".into()],
        );
        assert_eq!(excludes, vec!["tmp", "cache"]);
    }

    #[test]
    fn discover_tests_with_base_branch() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo_with_main(
            dir.path(),
            &[(
                "test_base.py",
                "from tryke import test\n\n@test\ndef test_base(): pass\n",
            )],
        );

        git_run(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(
            dir.path().join("test_feature.py"),
            "from tryke import test\n\n@test\ndef test_feature(): pass\n",
        )
        .expect("write");
        git_run(dir.path(), &["add", "test_feature.py"]);
        git_run(dir.path(), &["commit", "-m", "add feature test"]);

        let discovered = discover_tests(dir.path(), true, Some("main"), &[]);
        assert!(
            discovered.tests.iter().any(|t| t.name == "test_feature"),
            "should find the branch's test: {:?}",
            discovered.tests.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
    }

    // --- Changed-first tests ---

    #[test]
    fn discover_tests_changed_first_partitions_correctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(
            dir.path(),
            &[
                (
                    "test_a.py",
                    "from tryke import test\n\n@test\ndef test_a(): pass\n",
                ),
                (
                    "test_b.py",
                    "from tryke import test\n\n@test\ndef test_b(): pass\n",
                ),
            ],
        );

        // Modify test_a.py so it counts as "changed"
        std::fs::write(
            dir.path().join("test_a.py"),
            "from tryke import test\n\n@test\ndef test_a(): assert True\n",
        )
        .expect("write");

        let discovered = discover_tests_changed_first(dir.path(), None, &[]);
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();

        assert!(
            discovered.changed_prefix_len.is_some(),
            "changed_prefix_len should be set"
        );
        let prefix_len = discovered.changed_prefix_len.expect("set");
        assert!(prefix_len > 0, "at least one changed test");
        // Changed test(s) should be at the front
        let changed_names: Vec<&str> = names[..prefix_len].to_vec();
        assert!(
            changed_names.contains(&"test_a"),
            "test_a should be in the changed prefix: {names:?}"
        );
        // All tests should still be present
        assert!(
            names.contains(&"test_b"),
            "test_b should still be present: {names:?}"
        );
    }

    #[test]
    fn discover_tests_changed_first_no_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(
            dir.path(),
            &[(
                "test_a.py",
                "from tryke import test\n\n@test\ndef test_a(): pass\n",
            )],
        );

        let discovered = discover_tests_changed_first(dir.path(), None, &[]);
        assert!(
            discovered.changed_prefix_len.is_none(),
            "changed_prefix_len should be None when no changes"
        );
        assert!(
            !discovered.tests.is_empty(),
            "all tests should still be returned"
        );
    }

    #[test]
    fn discover_tests_changed_first_with_base_branch() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo_with_main(
            dir.path(),
            &[
                (
                    "test_a.py",
                    "from tryke import test\n\n@test\ndef test_a(): pass\n",
                ),
                (
                    "test_b.py",
                    "from tryke import test\n\n@test\ndef test_b(): pass\n",
                ),
            ],
        );

        git_run(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(
            dir.path().join("test_c.py"),
            "from tryke import test\n\n@test\ndef test_c(): pass\n",
        )
        .expect("write");
        git_run(dir.path(), &["add", "test_c.py"]);
        git_run(dir.path(), &["commit", "-m", "add test_c"]);

        let discovered = discover_tests_changed_first(dir.path(), Some("main"), &[]);
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();

        assert!(
            discovered.changed_prefix_len.is_some(),
            "changed_prefix_len should be set"
        );
        let prefix_len = discovered.changed_prefix_len.expect("set");
        // test_c should be in the changed prefix
        let changed_names: Vec<&str> = names[..prefix_len].to_vec();
        assert!(
            changed_names.contains(&"test_c"),
            "test_c should be in the changed prefix: {names:?}"
        );
        // All 3 tests should be present
        assert_eq!(names.len(), 3, "all 3 tests should be present: {names:?}");
    }

    #[test]
    fn discover_tests_includes_dynamic_import_warnings() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_dyn.py"),
            "import importlib\nmod = importlib.import_module('os')\nfrom tryke import test\n@test\ndef test_something():\n    pass\n",
        )
        .expect("write test_dyn.py");

        let discovered = discover_tests(dir.path(), false, None, &[]);
        assert!(
            !discovered.warnings.is_empty(),
            "should have at least one dynamic import warning"
        );
        let file_names: Vec<&str> = discovered
            .warnings
            .iter()
            .filter_map(|w| w.file_path.file_name())
            .filter_map(|n| n.to_str())
            .collect();
        assert!(
            file_names.contains(&"test_dyn.py"),
            "warning should reference test_dyn.py, got: {file_names:?}"
        );
    }

    // --- discover_tests_for_paths tests ---

    fn make_project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        for (rel, content) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create_dir_all");
            }
            std::fs::write(&path, content).expect("write file");
        }
        dir
    }

    fn pathspec_file(p: &str) -> PathSpec {
        PathSpec::File(PathBuf::from(p))
    }

    #[test]
    fn for_paths_single_file_finds_only_that_file_tests() {
        let dir = make_project(&[
            (
                "test_a.py",
                "from tryke import test\n@test\ndef test_a(): pass\n",
            ),
            (
                "test_b.py",
                "from tryke import test\n@test\ndef test_b(): pass\n",
            ),
        ]);
        let specs = vec![pathspec_file("test_a.py")];
        let discovered = discover_tests_for_paths(dir.path(), &specs, &[]);
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["test_a"], "got: {names:?}");
    }

    #[test]
    fn for_paths_directory_walks_only_subtree() {
        let dir = make_project(&[
            (
                "tests/test_a.py",
                "from tryke import test\n@test\ndef test_a(): pass\n",
            ),
            (
                "tests/test_b.py",
                "from tryke import test\n@test\ndef test_b(): pass\n",
            ),
            (
                "other/test_c.py",
                "from tryke import test\n@test\ndef test_c(): pass\n",
            ),
        ]);
        let specs = vec![pathspec_file("tests")];
        let discovered = discover_tests_for_paths(dir.path(), &specs, &[]);
        let mut names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["test_a", "test_b"], "got: {names:?}");
    }

    #[test]
    fn for_paths_mixed_file_and_dir_dedupe_by_ancestry() {
        let dir = make_project(&[
            (
                "tests/test_a.py",
                "from tryke import test\n@test\ndef test_a(): pass\n",
            ),
            (
                "tests/test_b.py",
                "from tryke import test\n@test\ndef test_b(): pass\n",
            ),
        ]);
        // Dir + a contained file should dedupe to just the dir; both
        // tests should be discovered (not just test_a).
        let specs = vec![pathspec_file("tests"), pathspec_file("tests/test_a.py")];
        let discovered = discover_tests_for_paths(dir.path(), &specs, &[]);
        let mut names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["test_a", "test_b"], "got: {names:?}");
    }

    #[test]
    fn for_paths_nonexistent_falls_back_to_full_walk() {
        let dir = make_project(&[(
            "test_real.py",
            "from tryke import test\n@test\ndef test_real(): pass\n",
        )]);
        let specs = vec![pathspec_file("does_not_exist.py")];
        let discovered = discover_tests_for_paths(dir.path(), &specs, &[]);
        // Fallback runs full discovery; the post-filter (applied in
        // main, not here) is what would narrow the set. So we expect
        // every test in the project here.
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"test_real"),
            "fallback should run full discovery: {names:?}"
        );
    }

    #[test]
    fn for_paths_file_line_spec_walks_just_that_file() {
        let dir = make_project(&[
            (
                "test_a.py",
                "from tryke import test\n@test\ndef test_a(): pass\n",
            ),
            (
                "test_b.py",
                "from tryke import test\n@test\ndef test_b(): pass\n",
            ),
        ]);
        let specs = vec![PathSpec::FileLine(PathBuf::from("test_a.py"), 2)];
        let discovered = discover_tests_for_paths(dir.path(), &specs, &[]);
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();
        // The walk is restricted to test_a.py — test_b should not appear
        // even before the post-filter narrows by line.
        assert_eq!(names, vec!["test_a"], "got: {names:?}");
    }

    #[test]
    fn for_paths_excludes_honored_inside_walk_root() {
        let dir = make_project(&[
            (
                "tests/test_a.py",
                "from tryke import test\n@test\ndef test_a(): pass\n",
            ),
            (
                "tests/skip/test_skipme.py",
                "from tryke import test\n@test\ndef test_skipme(): pass\n",
            ),
        ]);
        let specs = vec![pathspec_file("tests")];
        let discovered = discover_tests_for_paths(dir.path(), &specs, &["tests/skip".to_string()]);
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["test_a"], "got: {names:?}");
    }

    #[test]
    fn for_paths_outside_root_falls_back_to_full_walk() {
        let dir = make_project(&[(
            "test_real.py",
            "from tryke import test\n@test\ndef test_real(): pass\n",
        )]);
        let outside = tempfile::tempdir().expect("outside tempdir");
        let outside_file = outside.path().join("stray.py");
        std::fs::write(&outside_file, "x = 1\n").expect("write stray");
        let specs = vec![PathSpec::File(outside_file)];
        let discovered = discover_tests_for_paths(dir.path(), &specs, &[]);
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();
        // Out-of-root spec falls back to full discovery rather than
        // attempting to walk outside the project.
        assert!(
            names.contains(&"test_real"),
            "fallback should still find in-project tests: {names:?}"
        );
    }
}
