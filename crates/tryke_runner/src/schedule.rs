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
    partition_with_hooks(tests, &[], mode).units
}

/// The result of [`partition_with_hooks`]: the work units plus any
/// warnings produced while planning. Warnings cover situations where
/// the requested `mode` had to be upgraded for correctness (e.g. a
/// file-scope `per="scope"` fixture forced file-level grouping), and
/// should be surfaced to the user so they understand why the
/// distribution differs from what they asked for on the CLI.
#[derive(Debug)]
pub struct PartitionResult {
    pub units: Vec<WorkUnit>,
    pub warnings: Vec<String>,
}

/// Like [`partition`], but attaches discovered hooks to each work unit.
///
/// When `per="scope"` fixtures are present, `DistMode::Test` is upgraded:
/// - File-scope `per="scope"` fixtures (empty groups) force file-level grouping
/// - Group-scope `per="scope"` fixtures force group-level grouping
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
) -> PartitionResult {
    // Only consider hooks for modules that actually have tests in this run —
    // otherwise we warn about fixtures in files that were filtered out.
    let in_run_modules: std::collections::HashSet<&str> =
        tests.iter().map(|t| t.module_path.as_str()).collect();

    let constrained_modules: std::collections::HashSet<&str> = hooks
        .iter()
        .filter(|h| h.per.constrains_scheduling())
        .map(|h| h.module_path.as_str())
        .filter(|m| in_run_modules.contains(m))
        .collect();

    // Modules that have file-scope per-scope fixtures (empty groups) need
    // file-level grouping — the cached value must stay on one worker.
    let file_constrained_modules: std::collections::HashSet<&str> = hooks
        .iter()
        .filter(|h| h.per.constrains_scheduling() && h.groups.is_empty())
        .map(|h| h.module_path.as_str())
        .filter(|m| in_run_modules.contains(m))
        .collect();

    let mut warnings: Vec<String> = Vec::new();
    let mut units: Vec<WorkUnit> = if mode == DistMode::Test && !constrained_modules.is_empty() {
        // Test mode: upgrade constrained modules to group or file level.
        let (constrained, free): (Vec<_>, Vec<_>) = tests
            .into_iter()
            .partition(|t| constrained_modules.contains(t.module_path.as_str()));

        let mut affected: Vec<&str> = constrained_modules.iter().copied().collect();
        affected.sort_unstable();
        let upgraded_to = if file_constrained_modules.is_empty() {
            "group"
        } else {
            "file"
        };
        warnings.push(format!(
            "scheduler: upgrading --dist test → {upgraded_to} for {n} module(s) \
             because of per=\"scope\" fixtures ({mods}). Move the fixture into \
             a describe() to keep finer-grained distribution.",
            n = affected.len(),
            mods = affected.join(", "),
        ));

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
        // Group mode with file-scope per="scope" fixtures: upgrade those modules to file level.
        let (constrained, free): (Vec<_>, Vec<_>) = tests
            .into_iter()
            .partition(|t| file_constrained_modules.contains(t.module_path.as_str()));

        let mut affected: Vec<&str> = file_constrained_modules.iter().copied().collect();
        affected.sort_unstable();
        warnings.push(format!(
            "scheduler: upgrading --dist group → file for {n} module(s) \
             because of file-scope per=\"scope\" fixtures ({mods}). Move the \
             fixture into a describe() to keep group-level distribution.",
            n = affected.len(),
            mods = affected.join(", "),
        ));

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
    PartitionResult { units, warnings }
}

#[cfg(test)]
mod tests {
    use tryke_types::FixturePer;

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

    /// Default fixture helper — uses module "a" matching `item("a.py", ...)`.
    fn hook(name: &str, per: FixturePer, groups: &[&str]) -> HookItem {
        hook_for_module(name, "a", per, groups)
    }

    fn hook_for_module(name: &str, module: &str, per: FixturePer, groups: &[&str]) -> HookItem {
        HookItem {
            name: name.into(),
            module_path: module.into(),
            per,
            groups: groups.iter().map(|g| (*g).to_string()).collect(),
            depends_on: vec![],
            line_number: None,
        }
    }

    #[test]
    fn test_mode_with_file_scope_scope_fixture_forces_file_grouping() {
        let tests = vec![
            item("a.py", "t1", &[]),
            item("a.py", "t2", &[]),
            item("b.py", "t3", &[]),
        ];
        // Fixture is in test_mod (same as a.py items) — only a.py tests are constrained.
        let hooks = vec![hook("setup", FixturePer::Scope, &[])];
        let result = partition_with_hooks(tests, &hooks, DistMode::Test);
        let a_units: Vec<_> = result
            .units
            .iter()
            .filter(|u| u.tests.iter().any(|t| t.name == "t1"))
            .collect();
        assert_eq!(a_units.len(), 1);
        assert_eq!(a_units[0].tests.len(), 2, "a.py tests should be grouped");
    }

    #[test]
    fn test_mode_with_only_per_test_fixtures_stays_individual() {
        let tests = vec![item("a.py", "t1", &[]), item("a.py", "t2", &[])];
        let hooks = vec![hook("setup", FixturePer::Test, &[])];
        let result = partition_with_hooks(tests, &hooks, DistMode::Test);
        assert_eq!(
            result.units.len(),
            2,
            "per=\"test\" fixtures don't constrain scheduling"
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn work_unit_carries_hooks() {
        let tests = vec![item("a.py", "t1", &[])];
        let hooks = vec![hook("db", FixturePer::Scope, &[])];
        let result = partition_with_hooks(tests, &hooks, DistMode::Test);
        assert_eq!(result.units[0].hooks.len(), 1);
        assert_eq!(result.units[0].hooks[0].name, "db");
    }

    #[test]
    fn hooks_from_different_modules_only_attach_to_matching_units() {
        let tests = vec![item("a.py", "t1", &[]), item("b.py", "t2", &[])];
        let hooks = vec![
            hook_for_module("setup_a", "a", FixturePer::Test, &[]),
            hook_for_module("setup_b", "b", FixturePer::Test, &[]),
        ];
        let result = partition_with_hooks(tests, &hooks, DistMode::Test);
        assert_eq!(result.units.len(), 2);
        for u in &result.units {
            assert_eq!(u.hooks.len(), 1, "each unit should only have its own hook");
            let test_mod = &u.tests[0].module_path;
            assert_eq!(
                u.hooks[0].module_path, *test_mod,
                "hook module should match test module"
            );
        }
    }

    #[test]
    fn group_mode_with_file_scope_scope_fixture_forces_file_grouping() {
        let tests = vec![
            item("a.py", "t1", &["math"]),
            item("a.py", "t2", &["math"]),
            item("a.py", "t3", &["strings"]),
            item("b.py", "t4", &["other"]),
        ];
        // Module "a" has a file-scope per="scope" fixture (empty groups) — even in
        // group mode, all of a.py's tests must land on one worker.
        let hooks = vec![hook("setup", FixturePer::Scope, &[])];
        let result = partition_with_hooks(tests, &hooks, DistMode::Group);
        let a_units: Vec<_> = result
            .units
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
        let b_units: Vec<_> = result
            .units
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
        let hooks = vec![hook("setup", FixturePer::Scope, &["math"])];
        let result = partition_with_hooks(tests, &hooks, DistMode::Group);
        // Group mode keeps groups together already — no upgrade needed.
        // "math" (2 tests) and "strings" (1 test) stay separate.
        assert_eq!(result.units.len(), 2);
        assert!(result.warnings.is_empty(), "no upgrade → no warning");
    }

    #[test]
    fn scope_fixture_in_one_module_does_not_constrain_other_modules() {
        let tests = vec![
            item("a.py", "t1", &[]),
            item("a.py", "t2", &[]),
            item("b.py", "t3", &[]),
            item("b.py", "t4", &[]),
        ];
        // Only module "a" has a per="scope" fixture.
        let hooks = vec![hook_for_module("setup", "a", FixturePer::Scope, &[])];
        let result = partition_with_hooks(tests, &hooks, DistMode::Test);
        // a.py tests are grouped (1 unit), b.py tests are individual (2 units)
        let a_units: Vec<_> = result
            .units
            .iter()
            .filter(|u| u.tests.iter().any(|t| t.module_path == "a"))
            .collect();
        let b_units: Vec<_> = result
            .units
            .iter()
            .filter(|u| u.tests.iter().any(|t| t.module_path == "b"))
            .collect();
        assert_eq!(a_units.len(), 1, "a.py should be grouped into 1 unit");
        assert_eq!(a_units[0].tests.len(), 2);
        assert_eq!(b_units.len(), 2, "b.py tests should remain individual");
    }

    #[test]
    fn test_mode_upgrade_emits_warning_naming_affected_modules() {
        // The user asked for --dist test but a file-scope per="scope" fixture
        // on module "a" forces file-level grouping. The user must see this
        // as an explicit warning so they understand why their CLI flag
        // didn't take effect.
        let tests = vec![
            item("a.py", "t1", &[]),
            item("a.py", "t2", &[]),
            item("b.py", "t3", &[]),
        ];
        let hooks = vec![hook_for_module("setup", "a", FixturePer::Scope, &[])];
        let result = partition_with_hooks(tests, &hooks, DistMode::Test);
        assert_eq!(result.warnings.len(), 1, "expected one upgrade warning");
        let w = &result.warnings[0];
        assert!(w.contains("--dist test"), "warning text: {w}");
        assert!(w.contains("→ file"), "warning text: {w}");
        assert!(w.contains("(a)"), "warning should name module 'a': {w}");
    }

    #[test]
    fn group_mode_upgrade_emits_warning() {
        let tests = vec![
            item("a.py", "t1", &["math"]),
            item("a.py", "t2", &["strings"]),
        ];
        let hooks = vec![hook_for_module("setup", "a", FixturePer::Scope, &[])];
        let result = partition_with_hooks(tests, &hooks, DistMode::Group);
        assert_eq!(result.warnings.len(), 1);
        let w = &result.warnings[0];
        assert!(w.contains("--dist group"), "warning text: {w}");
        assert!(w.contains("→ file"), "warning text: {w}");
    }

    #[test]
    fn no_warnings_when_mode_is_not_upgraded() {
        let tests = vec![item("a.py", "t1", &[])];
        let result = partition_with_hooks(tests, &[], DistMode::Test);
        assert!(result.warnings.is_empty());
    }
}
