use std::path::Path;

use log::{debug, warn};
use tryke_config::load_effective_config;
use tryke_discovery::Discoverer;
use tryke_types::{DiscoveryWarning, DiscoveryWarningKind};

use crate::git::resolve_changed_files;

pub struct DiscoverySelection {
    pub tests: Vec<tryke_types::TestItem>,
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
    let warnings = dynamic_import_warnings(&discoverer);

    if changed {
        match resolve_changed_files(root, base_branch) {
            Some(changed_files) if !changed_files.is_empty() => {
                debug!("--changed: {} git-changed files", changed_files.len());
                DiscoverySelection {
                    tests: discoverer.tests_for_changed(&changed_files),
                    changed_files: Some(changed_files.len()),
                    changed_prefix_len: None,
                    warnings,
                }
            }
            Some(_) => {
                warn!("--changed: no changed files found via git, running all tests");
                DiscoverySelection {
                    tests: discoverer.tests(),
                    changed_files: None,
                    changed_prefix_len: None,
                    warnings,
                }
            }
            None => {
                warn!("--changed: git unavailable or failed, running all tests");
                DiscoverySelection {
                    tests: discoverer.tests(),
                    changed_files: None,
                    changed_prefix_len: None,
                    warnings,
                }
            }
        }
    } else {
        DiscoverySelection {
            tests: discoverer.tests(),
            changed_files: None,
            changed_prefix_len: None,
            warnings,
        }
    }
}

/// Discover all tests but place changed tests first in the returned list.
pub fn discover_tests_changed_first(
    root: &Path,
    base_branch: Option<&str>,
    excludes: &[String],
) -> DiscoverySelection {
    let mut discoverer = Discoverer::new_with_excludes(root, excludes);
    discoverer.rediscover();
    let warnings = dynamic_import_warnings(&discoverer);
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
                changed_files: Some(cf.len()),
                changed_prefix_len: Some(changed_prefix_len),
                warnings,
            }
        }
        Some(_) => {
            warn!("--changed-first: no changed files found, running all tests in default order");
            DiscoverySelection {
                tests: all_tests,
                changed_files: None,
                changed_prefix_len: None,
                warnings,
            }
        }
        None => {
            warn!("--changed-first: git unavailable, running all tests in default order");
            DiscoverySelection {
                tests: all_tests,
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
}
