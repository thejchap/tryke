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
pub fn partition_with_hooks(
    tests: Vec<TestItem>,
    hooks: &[HookItem],
    mode: DistMode,
) -> Vec<WorkUnit> {
    // Check if any _all hooks exist that constrain scheduling.
    let effective_mode =
        if mode == DistMode::Test && hooks.iter().any(|h| h.hook_type.constrains_scheduling()) {
            // File-scope _all hooks (empty groups) → force DistMode::File.
            // Group-scope _all hooks → force DistMode::Group.
            if hooks
                .iter()
                .any(|h| h.hook_type.constrains_scheduling() && h.groups.is_empty())
            {
                DistMode::File
            } else {
                DistMode::Group
            }
        } else {
            mode
        };

    let mut units: Vec<WorkUnit> = match effective_mode {
        DistMode::Test => tests
            .into_iter()
            .map(|t| WorkUnit {
                tests: vec![t],
                hooks: vec![],
            })
            .collect(),
        DistMode::File => {
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
        DistMode::Group => {
            let mut by_group: IndexMap<(Option<PathBuf>, Option<String>), Vec<TestItem>> =
                IndexMap::new();
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
    };
    // Attach hooks to each work unit.
    // For now, attach all provided hooks to every unit. The worker
    // filters by scope at runtime. This is refined in Step 6.
    if !hooks.is_empty() {
        for unit in &mut units {
            unit.hooks = hooks.to_vec();
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

    fn hook(name: &str, hook_type: HookType, groups: &[&str]) -> HookItem {
        HookItem {
            name: name.into(),
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
        let hooks = vec![hook("setup", HookType::BeforeAll, &[])];
        let units = partition_with_hooks(tests, &hooks, DistMode::Test);
        // a.py tests must be grouped (before_all constrains), b.py is separate
        // With file-scope before_all, all tests from a file go to one unit
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
}
