use std::path::{Path, PathBuf};

use log::{debug, warn};
use tryke_types::filter::PathSpec;
use tryke_types::{DiscoveryWarning, DiscoveryWarningKind, HookItem};

use crate::{Discoverer, git::resolve_changed_files};

pub struct DiscoverySelection {
    pub tests: Vec<tryke_types::TestItem>,
    /// Lifecycle hooks discovered alongside tests.
    pub hooks: Vec<HookItem>,
    pub changed_files: Option<usize>,
    /// Files where dynamic imports were detected; these will always re-run with --changed.
    pub warnings: Vec<DiscoveryWarning>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DiscoveryOptions<'a> {
    pub paths: &'a [PathSpec],
    pub changed: bool,
    pub changed_first: bool,
    pub base_branch: Option<&'a str>,
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

impl Discoverer {
    /// Discover tests using path and change-selection behavior from `options`.
    #[must_use]
    pub fn discover(&mut self, options: DiscoveryOptions<'_>) -> DiscoverySelection {
        if !options.paths.is_empty()
            && !options.changed
            && !options.changed_first
            && let Some(walk_roots) = resolve_walk_roots(self.root(), options.paths)
        {
            let tests = self.rediscover_restricted(&walk_roots);
            return self.selection(tests, None);
        }

        if !options.paths.is_empty() && !options.changed && !options.changed_first {
            debug!("Path-restricted discovery: falling back to full discovery");
        }

        let all_tests = self.rediscover();
        if options.changed_first {
            self.select_changed_first(all_tests, options.base_branch)
        } else if options.changed {
            self.select_changed(all_tests, options.base_branch)
        } else {
            self.selection(all_tests, None)
        }
    }

    fn select_changed(
        &self,
        all_tests: Vec<tryke_types::TestItem>,
        base_branch: Option<&str>,
    ) -> DiscoverySelection {
        match resolve_changed_files(self.root(), base_branch) {
            Some(changed_files) if !changed_files.is_empty() => {
                debug!("--changed: {} git-changed files", changed_files.len());
                self.selection(
                    self.tests_for_changed(&changed_files),
                    Some(changed_files.len()),
                )
            }
            Some(_) => {
                debug!("--changed: no changed files found via git, selecting nothing");
                self.selection(Vec::new(), Some(0))
            }
            None => {
                warn!("--changed: git unavailable or failed, running all tests");
                self.selection(all_tests, None)
            }
        }
    }

    fn select_changed_first(
        &self,
        all_tests: Vec<tryke_types::TestItem>,
        base_branch: Option<&str>,
    ) -> DiscoverySelection {
        match resolve_changed_files(self.root(), base_branch) {
            Some(changed_files) if !changed_files.is_empty() => {
                let changed_tests = self.tests_for_changed(&changed_files);
                let changed_ids: std::collections::HashSet<String> = changed_tests
                    .iter()
                    .map(tryke_types::TestItem::id)
                    .collect();
                let (first, rest): (Vec<_>, Vec<_>) = all_tests
                    .into_iter()
                    .partition(|test| changed_ids.contains(&test.id()));
                let mut tests = first;
                tests.extend(rest);
                self.selection(tests, Some(changed_files.len()))
            }
            Some(_) => {
                warn!(
                    "--changed-first: no changed files found, running all tests in default order"
                );
                self.selection(all_tests, None)
            }
            None => {
                warn!("--changed-first: git unavailable, running all tests in default order");
                self.selection(all_tests, None)
            }
        }
    }

    fn selection(
        &self,
        tests: Vec<tryke_types::TestItem>,
        changed_files: Option<usize>,
    ) -> DiscoverySelection {
        DiscoverySelection {
            tests,
            hooks: self.hooks(),
            changed_files,
            warnings: all_discovery_warnings(self),
        }
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
                "Path-restricted discovery: {} does not exist on disk",
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

#[cfg(test)]
mod tests {
    use tryke_config::{Project, ProjectMetadata, TrykeOptions};
    use tryke_testing::TestProject;

    use super::*;
    use crate::git::test_helpers::*;

    fn discover_project(project: &Project, options: DiscoveryOptions<'_>) -> DiscoverySelection {
        Discoverer::new(project).discover(options)
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

        let project = Project::discover(dir.path());
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                changed: true,
                base_branch: Some("main"),
                ..DiscoveryOptions::default()
            },
        );
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

        let project = Project::discover(dir.path());
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                changed_first: true,
                ..DiscoveryOptions::default()
            },
        );
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();

        assert_eq!(
            names.first(),
            Some(&"test_a"),
            "test_a should be first: {names:?}"
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

        let project = Project::discover(dir.path());
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                changed_first: true,
                ..DiscoveryOptions::default()
            },
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

        let project = Project::discover(dir.path());
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                changed_first: true,
                base_branch: Some("main"),
                ..DiscoveryOptions::default()
            },
        );
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();

        assert_eq!(
            names.first(),
            Some(&"test_c"),
            "test_c should be first: {names:?}"
        );
        // All 3 tests should be present
        assert_eq!(names.len(), 3, "all 3 tests should be present: {names:?}");
    }

    #[test]
    fn discover_tests_includes_dynamic_import_warnings() {
        let dir = TestProject::with_files([(
            "test_dyn.py",
            "import importlib\nmod = importlib.import_module('os')\nfrom tryke import test\n@test\ndef test_something():\n    pass\n",
        )])
        .expect("create test project");

        let project = dir.project();
        let discovered = discover_project(&project, DiscoveryOptions::default());
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

    // --- Path-restricted discovery tests ---

    fn make_project(files: &[(&str, &str)]) -> TestProject {
        TestProject::with_files(files.iter().copied()).expect("create test project")
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
        let project = dir.project();
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                paths: &specs,
                ..DiscoveryOptions::default()
            },
        );
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
        let project = dir.project();
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                paths: &specs,
                ..DiscoveryOptions::default()
            },
        );
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
        let project = dir.project();
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                paths: &specs,
                ..DiscoveryOptions::default()
            },
        );
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
        let project = dir.project();
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                paths: &specs,
                ..DiscoveryOptions::default()
            },
        );
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
        let project = dir.project();
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                paths: &specs,
                ..DiscoveryOptions::default()
            },
        );
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
        let mut metadata = ProjectMetadata::new(dir.root());
        metadata.apply_configuration_file();
        metadata.apply_cli_args(TrykeOptions {
            exclude: Some(vec!["tests/skip".to_string()]),
            ..TrykeOptions::default()
        });
        let project = Project::from_metadata(metadata);
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                paths: &specs,
                ..DiscoveryOptions::default()
            },
        );
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
        let project = dir.project();
        let discovered = discover_project(
            &project,
            DiscoveryOptions {
                paths: &specs,
                ..DiscoveryOptions::default()
            },
        );
        let names: Vec<&str> = discovered.tests.iter().map(|t| t.name.as_str()).collect();
        // Out-of-root spec falls back to full discovery rather than
        // attempting to walk outside the project.
        assert!(
            names.contains(&"test_real"),
            "fallback should still find in-project tests: {names:?}"
        );
    }
}
