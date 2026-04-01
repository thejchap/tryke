use std::path::PathBuf;

use indexmap::IndexMap;
use tryke_types::{HookItem, TestItem};

/// How tests are partitioned into work units for distribution across workers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DistMode {
    /// Each test is its own work unit. Maximum parallelism.
    /// Tests from the same file may run on different workers.
    #[default]
    Test,
    /// All tests from a file go to one worker. Module state is
    /// deterministic within a file. Tests run sequentially per file.
    File,
    /// Tests within a `describe()` group go to one worker.
    /// Different groups from the same file may run on different workers.
    Group,
}

/// A batch of tests assigned to one worker as an atomic unit.
/// The worker runs all tests in the batch before pulling another unit.
#[derive(Debug)]
pub struct WorkUnit {
    pub tests: Vec<TestItem>,
    /// Hooks relevant to the tests in this unit, sent once before execution.
    pub hooks: Vec<HookItem>,
}

/// Partition a flat list of tests into work units according to `mode`,
/// then sort largest-first for optimal load balancing.
///
/// When `hooks` are provided, each unit receives the hooks relevant to
/// its tests (matched by file path).
#[must_use]
pub fn partition(tests: Vec<TestItem>, mode: DistMode) -> Vec<WorkUnit> {
    partition_with_hooks(tests, &[], mode)
}

/// Like [`partition`], but attaches discovered hooks to each work unit.
///
/// When `_all` hooks are present, `DistMode::Test` is upgraded:
/// - File-scope `_all` hooks (empty groups) force file-level grouping
/// - Group-scope `_all` hooks force group-level grouping
#[must_use]
fn group_by_file(tests: Vec<TestItem>) -> Vec<WorkUnit> {
    let mut by_file: IndexMap<Option<PathBuf>, Vec<TestItem>> = IndexMap::new();
    for t in tests {
        by_file.entry(t.file_path.clone()).or_default().push(t);
    }
    by_file
        .into_values()
        .map(|tests| WorkUnit {
            tests,
            hooks: vec![],
        })
        .collect()
}

fn group_by_describe(tests: Vec<TestItem>) -> Vec<WorkUnit> {
    let mut by_group: IndexMap<(Option<PathBuf>, Option<String>), Vec<TestItem>> = IndexMap::new();
    for t in tests {
        let group_key = t.groups.first().cloned();
        by_group
            .entry((t.file_path.clone(), group_key))
            .or_default()
            .push(t);
    }
    by_group
        .into_values()
        .map(|tests| WorkUnit {
            tests,
            hooks: vec![],
        })
        .collect()
}

#[must_use]
pub fn partition_with_hooks(
    tests: Vec<TestItem>,
    hooks: &[HookItem],
    mode: DistMode,
) -> Vec<WorkUnit> {
    let constrained_modules: std::collections::HashSet<&str> = hooks
        .iter()
        .filter(|h| h.hook_type.constrains_scheduling())
        .map(|h| h.module_path.as_str())
        .collect();

    // Modules that have file-scope _all hooks (empty groups) need file-level grouping.
    let file_constrained_modules: std::collections::HashSet<&str> = hooks
        .iter()
        .filter(|h| h.hook_type.constrains_scheduling() && h.groups.is_empty())
        .map(|h| h.module_path.as_str())
        .collect();

    let mut units: Vec<WorkUnit> = if mode == DistMode::Test && !constrained_modules.is_empty() {
        // Test mode: upgrade constrained modules to group or file level.
        let (constrained, free): (Vec<_>, Vec<_>) = tests
            .into_iter()
            .partition(|t| constrained_modules.contains(t.module_path.as_str()));

        let mut result: Vec<WorkUnit> = free
            .into_iter()
            .map(|t| WorkUnit {
                tests: vec![t],
                hooks: vec![],
            })
            .collect();

        if file_constrained_modules.is_empty() {
            result.extend(group_by_describe(constrained));
        } else {
            result.extend(group_by_file(constrained));
        }
        result
    } else if mode == DistMode::Group && !file_constrained_modules.is_empty() {
        // Group mode with file-scope _all hooks: upgrade those modules to file level.
        let (constrained, free): (Vec<_>, Vec<_>) = tests
            .into_iter()
            .partition(|t| file_constrained_modules.contains(t.module_path.as_str()));

        let mut result = group_by_describe(free);
        result.extend(group_by_file(constrained));
        result
    } else {
        match mode {
            DistMode::Test => tests
                .into_iter()
                .map(|t| WorkUnit {
                    tests: vec![t],
                    hooks: vec![],
                })
                .collect(),
            DistMode::File => group_by_file(tests),
            DistMode::Group => group_by_describe(tests),
        }
    };

    // Attach hooks to each unit, filtered by the modules in that unit.
    if !hooks.is_empty() {
        for unit in &mut units {
            let unit_modules: std::collections::HashSet<&str> =
                unit.tests.iter().map(|t| t.module_path.as_str()).collect();
            unit.hooks = hooks
                .iter()
                .filter(|h| unit_modules.contains(h.module_path.as_str()))
                .cloned()
                .collect();
        }
    }

    // Largest units first: longest-pole-first scheduling minimises tail latency.
    units.sort_by(|a, b| b.tests.len().cmp(&a.tests.len()));
    units
}

#[cfg(test)]
mod tests {
    use tryke_types::HookType;

    use super::*;

    fn item(file: &str, name: &str, groups: &[&str]) -> TestItem {
        TestItem {
            name: name.into(),
            module_path: file.replace('/', ".").replace(".py", ""),
            file_path: Some(PathBuf::from(file)),
            groups: groups.iter().map(|g| (*g).to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_mode_one_unit_per_test() {
        let tests = vec![
            item("a.py", "t1", &[]),
            item("a.py", "t2", &[]),
            item("b.py", "t3", &[]),
        ];
        let units = partition(tests, DistMode::Test);
        assert_eq!(units.len(), 3);
        for u in &units {
            assert_eq!(u.tests.len(), 1);
        }
    }

    #[test]
    fn file_mode_groups_by_file() {
        let tests = vec![
            item("a.py", "t1", &[]),
            item("a.py", "t2", &[]),
            item("b.py", "t3", &[]),
        ];
        let units = partition(tests, DistMode::File);
        assert_eq!(units.len(), 2);
        // Largest first: a.py has 2 tests, b.py has 1
        assert_eq!(units[0].tests.len(), 2);
        assert_eq!(units[0].tests[0].name, "t1");
        assert_eq!(units[0].tests[1].name, "t2");
        assert_eq!(units[1].tests.len(), 1);
        assert_eq!(units[1].tests[0].name, "t3");
    }

    #[test]
    fn group_mode_splits_by_describe() {
        let tests = vec![
            item("a.py", "t1", &["math"]),
            item("a.py", "t2", &["math"]),
            item("a.py", "t3", &["strings"]),
            item("a.py", "t4", &[]), // No group
        ];
        let units = partition(tests, DistMode::Group);
        assert_eq!(units.len(), 3);
        // Largest first: "math" group has 2 tests
        assert_eq!(units[0].tests.len(), 2);
        assert!(units[0].tests.iter().all(|t| t.groups == vec!["math"]));
    }

    #[test]
    fn file_mode_preserves_discovery_order_within_file() {
        let tests = vec![
            item("a.py", "t1", &[]),
            item("a.py", "t2", &[]),
            item("a.py", "t3", &[]),
        ];
        let units = partition(tests, DistMode::File);
        assert_eq!(units.len(), 1);
        let names: Vec<&str> = units[0].tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["t1", "t2", "t3"]);
    }

    #[test]
    fn largest_units_sort_first() {
        let tests = vec![
            item("small.py", "t1", &[]),
            item("big.py", "t1", &[]),
            item("big.py", "t2", &[]),
            item("big.py", "t3", &[]),
            item("medium.py", "t1", &[]),
            item("medium.py", "t2", &[]),
        ];
        let units = partition(tests, DistMode::File);
        assert_eq!(units.len(), 3);
        assert_eq!(units[0].tests.len(), 3); // big.py
        assert_eq!(units[1].tests.len(), 2); // medium.py
        assert_eq!(units[2].tests.len(), 1); // small.py
    }

    /// Default hook helper — uses module "a" matching `item("a.py", ...)`.
    fn hook(name: &str, hook_type: HookType, groups: &[&str]) -> HookItem {
        hook_for_module(name, "a", hook_type, groups)
    }

    fn hook_for_module(name: &str, module: &str, hook_type: HookType, groups: &[&str]) -> HookItem {
        HookItem {
            name: name.into(),
            module_path: module.into(),
            hook_type,
            groups: groups.iter().map(|g| (*g).to_string()).collect(),
            depends_on: vec![],
            line_number: None,
        }
    }

    #[test]
    fn test_mode_with_file_scope_before_all_forces_file_grouping() {
        let tests = vec![
            item("a.py", "t1", &[]),
            item("a.py", "t2", &[]),
            item("b.py", "t3", &[]),
        ];
        // Hook is in test_mod (same as a.py items) — only a.py tests are constrained.
        let hooks = vec![hook("setup", HookType::BeforeAll, &[])];
        let units = partition_with_hooks(tests, &hooks, DistMode::Test);
        let a_units: Vec<_> = units
            .iter()
            .filter(|u| u.tests.iter().any(|t| t.name == "t1"))
            .collect();
        assert_eq!(a_units.len(), 1);
        assert_eq!(a_units[0].tests.len(), 2, "a.py tests should be grouped");
    }

    #[test]
    fn test_mode_with_only_each_hooks_stays_individual() {
        let tests = vec![item("a.py", "t1", &[]), item("a.py", "t2", &[])];
        let hooks = vec![hook("setup", HookType::BeforeEach, &[])];
        let units = partition_with_hooks(tests, &hooks, DistMode::Test);
        assert_eq!(units.len(), 2, "each hooks don't constrain scheduling");
    }

    #[test]
    fn work_unit_carries_hooks() {
        let tests = vec![item("a.py", "t1", &[])];
        let hooks = vec![hook("db", HookType::BeforeAll, &[])];
        let units = partition_with_hooks(tests, &hooks, DistMode::Test);
        assert_eq!(units[0].hooks.len(), 1);
        assert_eq!(units[0].hooks[0].name, "db");
    }

    #[test]
    fn hooks_from_different_modules_only_attach_to_matching_units() {
        let tests = vec![item("a.py", "t1", &[]), item("b.py", "t2", &[])];
        let hooks = vec![
            hook_for_module("setup_a", "a", HookType::BeforeEach, &[]),
            hook_for_module("setup_b", "b", HookType::BeforeEach, &[]),
        ];
        let units = partition_with_hooks(tests, &hooks, DistMode::Test);
        assert_eq!(units.len(), 2);
        for u in &units {
            assert_eq!(u.hooks.len(), 1, "each unit should only have its own hook");
            let test_mod = &u.tests[0].module_path;
            assert_eq!(
                u.hooks[0].module_path, *test_mod,
                "hook module should match test module"
            );
        }
    }

    #[test]
    fn group_mode_with_file_scope_before_all_forces_file_grouping() {
        let tests = vec![
            item("a.py", "t1", &["math"]),
            item("a.py", "t2", &["math"]),
            item("a.py", "t3", &["strings"]),
            item("b.py", "t4", &["other"]),
        ];
        // Module "a" has a file-scope before_all (empty groups) — even in group mode,
        // all of a.py's tests must land on one worker.
        let hooks = vec![hook("setup", HookType::BeforeAll, &[])];
        let units = partition_with_hooks(tests, &hooks, DistMode::Group);
        let a_units: Vec<_> = units
            .iter()
            .filter(|u| u.tests.iter().any(|t| t.module_path == "a"))
            .collect();
        assert_eq!(
            a_units.len(),
            1,
            "a.py should be grouped into 1 unit despite --dist group"
        );
        assert_eq!(a_units[0].tests.len(), 3);
        // b.py is unconstrained — stays as its own group.
        let b_units: Vec<_> = units
            .iter()
            .filter(|u| u.tests.iter().any(|t| t.module_path == "b"))
            .collect();
        assert_eq!(b_units.len(), 1);
    }

    #[test]
    fn group_mode_with_only_describe_scoped_hooks_stays_group() {
        let tests = vec![
            item("a.py", "t1", &["math"]),
            item("a.py", "t2", &["math"]),
            item("a.py", "t3", &["strings"]),
        ];
        // Hook is scoped to "math" group only — no file-scope constraint.
        let hooks = vec![hook("setup", HookType::BeforeAll, &["math"])];
        let units = partition_with_hooks(tests, &hooks, DistMode::Group);
        // Group mode keeps groups together already — no upgrade needed.
        // "math" (2 tests) and "strings" (1 test) stay separate.
        assert_eq!(units.len(), 2);
    }

    #[test]
    fn before_all_in_one_module_does_not_constrain_other_modules() {
        let tests = vec![
            item("a.py", "t1", &[]),
            item("a.py", "t2", &[]),
            item("b.py", "t3", &[]),
            item("b.py", "t4", &[]),
        ];
        // Only module "a" has a before_all hook.
        let hooks = vec![hook_for_module("setup", "a", HookType::BeforeAll, &[])];
        let units = partition_with_hooks(tests, &hooks, DistMode::Test);
        // a.py tests are grouped (1 unit), b.py tests are individual (2 units)
        let a_units: Vec<_> = units
            .iter()
            .filter(|u| u.tests.iter().any(|t| t.module_path == "a"))
            .collect();
        let b_units: Vec<_> = units
            .iter()
            .filter(|u| u.tests.iter().any(|t| t.module_path == "b"))
            .collect();
        assert_eq!(a_units.len(), 1, "a.py should be grouped into 1 unit");
        assert_eq!(a_units[0].tests.len(), 2);
        assert_eq!(b_units.len(), 2, "b.py tests should remain individual");
    }
}
