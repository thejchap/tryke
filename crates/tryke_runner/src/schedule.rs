use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

use indexmap::IndexMap;
use tryke_types::{DiscoveryWarning, DiscoveryWarningKind, HookItem, TestItem, TestRunHistory};

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

/// The result of the scheduler's allocation of a set of tests to a worker
#[derive(Debug)]
pub struct PartitionResult {
    /// A set of `WorkUnit`s allocated to a worker
    ///
    /// `WorkUnit` is a set of tests to be run together
    pub units: Vec<WorkUnit>,

    /// Warnings produced while planning. Warnings cover situations where
    /// the requested `mode` had to be upgraded for correctness (e.g. a
    /// file-scope `per="scope"` fixture forced file-level grouping), and
    /// should be surfaced to the user so they understand why the
    /// distribution differs from what they asked for on the CLI.
    pub warnings: Vec<DiscoveryWarning>,
}

/// Ordering hints for [`plan_work_units`]. Both maps are keyed by
/// `TestItem::id()`; IDs absent from the run are ignored.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScheduleOptions<'a> {
    /// Last observed outcome/duration per test.
    pub history: Option<&'a HashMap<String, TestRunHistory>>,
    /// Tests whose units must run before all others (`--changed-first`).
    pub first: Option<&'a HashSet<String>>,
}

/// Groups tests by file, pairing each unit with the discovery position of
/// its first member.
#[must_use]
fn group_by_file(tests: impl IntoIterator<Item = (usize, TestItem)>) -> Vec<(usize, WorkUnit)> {
    let mut by_file: IndexMap<Option<PathBuf>, (usize, Vec<TestItem>)> = IndexMap::new();
    for (position, t) in tests {
        by_file
            .entry(t.file_path.clone())
            .or_insert_with(|| (position, Vec::new()))
            .1
            .push(t);
    }
    by_file
        .into_values()
        .map(|(position, tests)| {
            (
                position,
                WorkUnit {
                    tests,
                    hooks: vec![],
                },
            )
        })
        .collect()
}

/// A test's file and top-level `describe()` group.
type DescribeKey = (Option<PathBuf>, Option<String>);

/// Groups tests by file and top-level describe, pairing each unit with the
/// discovery position of its first member.
fn group_by_describe(tests: impl IntoIterator<Item = (usize, TestItem)>) -> Vec<(usize, WorkUnit)> {
    let mut by_group: IndexMap<DescribeKey, (usize, Vec<TestItem>)> = IndexMap::new();
    for (position, t) in tests {
        let group_key = t.groups.first().cloned();
        by_group
            .entry((t.file_path.clone(), group_key))
            .or_insert_with(|| (position, Vec::new()))
            .1
            .push(t);
    }
    by_group
        .into_values()
        .map(|(position, tests)| {
            (
                position,
                WorkUnit {
                    tests,
                    hooks: vec![],
                },
            )
        })
        .collect()
}

fn singleton(position: usize, test: TestItem) -> (usize, WorkUnit) {
    (
        position,
        WorkUnit {
            tests: vec![test],
            hooks: vec![],
        },
    )
}

/// Build atomic work units from `tests` according to `mode`, attach the
/// hooks relevant to each unit (matched by module), and order the units.
///
/// When `per="scope"` fixtures are present, `DistMode::Test` is upgraded:
/// - File-scope `per="scope"` fixtures (empty groups) force file-level grouping
/// - Group-scope `per="scope"` fixtures force group-level grouping
///
/// Units are ordered by: explicit priority (`options.first`), then any
/// previously failing test, then any test without a measured duration,
/// then descending estimated work (summed durations for fully measured
/// units, test count for units with unknown timings). Ties keep the
/// original discovery position of each unit's first member, and tests
/// inside a unit are never reordered.
#[must_use]
pub fn plan_work_units(
    tests: Vec<TestItem>,
    hooks: &[HookItem],
    mode: DistMode,
    options: ScheduleOptions<'_>,
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

    let mut warnings: Vec<DiscoveryWarning> = Vec::new();
    let indexed = tests.into_iter().enumerate();
    let mut units: Vec<(usize, WorkUnit)> =
        if mode == DistMode::Test && !constrained_modules.is_empty() {
            // Test mode: upgrade constrained modules to group or file level.
            let (constrained, free): (Vec<_>, Vec<_>) =
                indexed.partition(|(_, t)| constrained_modules.contains(t.module_path.as_str()));

            let mut affected: Vec<&str> = constrained_modules.iter().copied().collect();
            affected.sort_unstable();
            let upgraded_to = if file_constrained_modules.is_empty() {
                "group"
            } else {
                "file"
            };
            warnings.push(DiscoveryWarning {
                file_path: PathBuf::new(),
                kind: DiscoveryWarningKind::DistModeUpgrade,
                message: format!(
                    "scheduler: upgrading --dist test → {upgraded_to} for {n} module(s) \
                 because of per=\"scope\" fixtures ({mods}). Move the fixture into \
                 a describe() to keep finer-grained distribution.",
                    n = affected.len(),
                    mods = affected.join(", "),
                ),
            });

            let mut result: Vec<(usize, WorkUnit)> = free
                .into_iter()
                .map(|(position, t)| singleton(position, t))
                .collect();

            if file_constrained_modules.is_empty() {
                result.extend(group_by_describe(constrained));
            } else {
                result.extend(group_by_file(constrained));
            }
            result
        } else if mode == DistMode::Group && !file_constrained_modules.is_empty() {
            // Group mode with file-scope per="scope" fixtures: upgrade those modules to file level.
            let (constrained, free): (Vec<_>, Vec<_>) = indexed
                .partition(|(_, t)| file_constrained_modules.contains(t.module_path.as_str()));

            let mut affected: Vec<&str> = file_constrained_modules.iter().copied().collect();
            affected.sort_unstable();
            warnings.push(DiscoveryWarning {
                file_path: PathBuf::new(),
                kind: DiscoveryWarningKind::DistModeUpgrade,
                message: format!(
                    "scheduler: upgrading --dist group → file for {n} module(s) \
                 because of file-scope per=\"scope\" fixtures ({mods}). Move the \
                 fixture into a describe() to keep group-level distribution.",
                    n = affected.len(),
                    mods = affected.join(", "),
                ),
            });

            let mut result = group_by_describe(free);
            result.extend(group_by_file(constrained));
            result
        } else {
            match mode {
                DistMode::Test => indexed
                    .map(|(position, t)| singleton(position, t))
                    .collect(),
                DistMode::File => group_by_file(indexed),
                DistMode::Group => group_by_describe(indexed),
            }
        };

    // Attach hooks to each unit, filtered by the modules in that unit.
    if !hooks.is_empty() {
        for (_, unit) in &mut units {
            let unit_modules: std::collections::HashSet<&str> =
                unit.tests.iter().map(|t| t.module_path.as_str()).collect();
            unit.hooks = hooks
                .iter()
                .filter(|h| unit_modules.contains(h.module_path.as_str()))
                .cloned()
                .collect();
        }
    }

    // The discovery position is only a final tie-breaker: fixture upgrades
    // build free units before constrained ones, so construction order is
    // not discovery order.
    units.sort_by_cached_key(|(position, unit)| (unit_rank(unit, options), *position));
    let units = units.into_iter().map(|(_, unit)| unit).collect();
    PartitionResult { units, warnings }
}

/// Ascending sort key: `(!priority, !failure, !unknown, Reverse(known
/// total), Reverse(cold count))`. Only runnable tests contribute: a
/// skip/todo member can't promote a unit or make its timing unknown.
fn unit_rank(
    unit: &WorkUnit,
    options: ScheduleOptions<'_>,
) -> (bool, bool, bool, Reverse<Duration>, Reverse<usize>) {
    let history = options.history.filter(|history| !history.is_empty());
    let first = options.first.filter(|first| !first.is_empty());
    let mut runnable = unit
        .tests
        .iter()
        .filter(|test| test.skip.is_none() && test.todo.is_none());

    let mut has_priority = false;
    let mut has_failure = false;
    let mut has_unknown = false;
    let mut known_total = Duration::ZERO;
    if history.is_none() && first.is_none() {
        has_unknown = runnable.next().is_some();
    } else {
        for test in runnable {
            let id = test.id();
            has_priority |= first.is_some_and(|first| first.contains(&id));
            match history.and_then(|history| history.get(&id)) {
                Some(record) => {
                    has_failure |= record.failed;
                    match record.duration {
                        Some(duration) => known_total = known_total.saturating_add(duration),
                        None => has_unknown = true,
                    }
                }
                None => has_unknown = true,
            }
        }
    }

    let (known_total, cold_count) = if has_unknown {
        (Duration::ZERO, unit.tests.len())
    } else {
        (known_total, 0)
    };
    (
        !has_priority,
        !has_failure,
        !has_unknown,
        Reverse(known_total),
        Reverse(cold_count),
    )
}

#[cfg(test)]
mod tests {
    use tryke_types::{DiscoveryWarningKind, FixturePer};

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
        let units = plan_work_units(tests, &[], DistMode::Test, ScheduleOptions::default()).units;
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
        let units = plan_work_units(tests, &[], DistMode::File, ScheduleOptions::default()).units;
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
        let units = plan_work_units(tests, &[], DistMode::Group, ScheduleOptions::default()).units;
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
        let units = plan_work_units(tests, &[], DistMode::File, ScheduleOptions::default()).units;
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
        let units = plan_work_units(tests, &[], DistMode::File, ScheduleOptions::default()).units;
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
        let result = plan_work_units(tests, &hooks, DistMode::Test, ScheduleOptions::default());
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
        let result = plan_work_units(tests, &hooks, DistMode::Test, ScheduleOptions::default());
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
        let result = plan_work_units(tests, &hooks, DistMode::Test, ScheduleOptions::default());
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
        let result = plan_work_units(tests, &hooks, DistMode::Test, ScheduleOptions::default());
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
        let result = plan_work_units(tests, &hooks, DistMode::Group, ScheduleOptions::default());
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
        let result = plan_work_units(tests, &hooks, DistMode::Group, ScheduleOptions::default());
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
        let result = plan_work_units(tests, &hooks, DistMode::Test, ScheduleOptions::default());
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
        let result = plan_work_units(tests, &hooks, DistMode::Test, ScheduleOptions::default());
        assert_eq!(result.warnings.len(), 1, "expected one upgrade warning");
        let w = &result.warnings[0];
        assert_eq!(w.kind, DiscoveryWarningKind::DistModeUpgrade);
        assert!(w.message.contains("--dist test"), "warning text: {w:?}");
        assert!(w.message.contains("→ file"), "warning text: {w:?}");
        assert!(
            w.message.contains("(a)"),
            "warning should name module 'a': {w:?}"
        );
    }

    #[test]
    fn group_mode_upgrade_emits_warning() {
        let tests = vec![
            item("a.py", "t1", &["math"]),
            item("a.py", "t2", &["strings"]),
        ];
        let hooks = vec![hook_for_module("setup", "a", FixturePer::Scope, &[])];
        let result = plan_work_units(tests, &hooks, DistMode::Group, ScheduleOptions::default());
        assert_eq!(result.warnings.len(), 1);
        let w = &result.warnings[0];
        assert_eq!(w.kind, DiscoveryWarningKind::DistModeUpgrade);
        assert!(w.message.contains("--dist group"), "warning text: {w:?}");
        assert!(w.message.contains("→ file"), "warning text: {w:?}");
    }

    #[test]
    fn no_warnings_when_mode_is_not_upgraded() {
        let tests = vec![item("a.py", "t1", &[])];
        let result = plan_work_units(tests, &[], DistMode::Test, ScheduleOptions::default());
        assert!(result.warnings.is_empty());
    }

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn history(records: &[(&TestItem, bool, Option<Duration>)]) -> HashMap<String, TestRunHistory> {
        records
            .iter()
            .map(|(test, failed, duration)| {
                (
                    test.id(),
                    TestRunHistory {
                        failed: *failed,
                        duration: *duration,
                    },
                )
            })
            .collect()
    }

    fn plan(
        tests: Vec<TestItem>,
        mode: DistMode,
        history: &HashMap<String, TestRunHistory>,
        first: &HashSet<String>,
    ) -> Vec<WorkUnit> {
        plan_work_units(
            tests,
            &[],
            mode,
            ScheduleOptions {
                history: Some(history),
                first: Some(first),
            },
        )
        .units
    }

    fn unit_ids(units: &[WorkUnit]) -> Vec<Vec<String>> {
        units
            .iter()
            .map(|unit| unit.tests.iter().map(TestItem::id).collect())
            .collect()
    }

    fn ids(tests: &[&TestItem]) -> Vec<String> {
        tests.iter().map(|test| test.id()).collect()
    }

    #[test]
    fn ranks_priority_then_failures_then_unknown_then_longest() {
        let slow = item("slow.py", "t", &[]);
        let failed = item("failed.py", "t", &[]);
        let unknown = item("unknown.py", "t", &[]);
        let fast = item("fast.py", "t", &[]);
        let history = history(&[
            (&slow, false, Some(ms(900))),
            (&failed, true, Some(ms(10))),
            (&fast, false, Some(ms(100))),
        ]);
        let tests = vec![slow.clone(), failed.clone(), unknown.clone(), fast.clone()];

        let units = plan(tests.clone(), DistMode::Test, &history, &HashSet::new());
        assert_eq!(
            unit_ids(&units),
            vec![
                ids(&[&failed]),
                ids(&[&unknown]),
                ids(&[&slow]),
                ids(&[&fast])
            ]
        );

        let first = HashSet::from([fast.id()]);
        let units = plan(tests, DistMode::Test, &history, &first);
        assert_eq!(
            unit_ids(&units),
            vec![
                ids(&[&fast]),
                ids(&[&failed]),
                ids(&[&unknown]),
                ids(&[&slow])
            ]
        );
    }

    #[test]
    fn failing_record_without_duration_precedes_unknown() {
        let unknown = item("unknown.py", "t", &[]);
        let failed = item("failed.py", "t", &[]);
        let history = history(&[(&failed, true, None)]);
        let units = plan(
            vec![unknown.clone(), failed.clone()],
            DistMode::Test,
            &history,
            &HashSet::new(),
        );
        assert_eq!(unit_ids(&units), vec![ids(&[&failed]), ids(&[&unknown])]);
    }

    #[test]
    fn grouped_units_rank_by_summed_duration() {
        let a1 = item("a.py", "t1", &[]);
        let a2 = item("a.py", "t2", &[]);
        let b1 = item("b.py", "t1", &[]);
        let file_history = history(&[
            (&a1, false, Some(ms(10))),
            (&a2, false, Some(ms(10))),
            (&b1, false, Some(ms(100))),
        ]);
        let units = plan(
            vec![a1.clone(), a2.clone(), b1.clone()],
            DistMode::File,
            &file_history,
            &HashSet::new(),
        );
        assert_eq!(unit_ids(&units), vec![ids(&[&b1]), ids(&[&a1, &a2])]);

        let m1 = item("g.py", "t1", &["math"]);
        let m2 = item("g.py", "t2", &["math"]);
        let s1 = item("g.py", "t3", &["strings"]);
        let group_history = history(&[
            (&m1, false, Some(ms(10))),
            (&m2, false, Some(ms(10))),
            (&s1, false, Some(ms(100))),
        ]);
        let units = plan(
            vec![m1.clone(), m2.clone(), s1.clone()],
            DistMode::Group,
            &group_history,
            &HashSet::new(),
        );
        assert_eq!(unit_ids(&units), vec![ids(&[&s1]), ids(&[&m1, &m2])]);
    }

    #[test]
    fn partially_known_unit_ranks_as_unknown_by_test_count() {
        let c1 = item("c.py", "t1", &[]);
        let d1 = item("d.py", "t1", &[]);
        let d2 = item("d.py", "t2", &[]);
        let a1 = item("a.py", "t1", &[]);
        let a2 = item("a.py", "t2", &[]);
        let b1 = item("b.py", "t1", &[]);
        let b2 = item("b.py", "t2", &[]);
        let b3 = item("b.py", "t3", &[]);
        let history = history(&[
            (&c1, false, Some(Duration::from_hours(1))),
            (&a1, false, Some(Duration::from_hours(1))),
        ]);
        let units = plan(
            vec![
                c1.clone(),
                d1.clone(),
                d2.clone(),
                a1.clone(),
                a2.clone(),
                b1.clone(),
                b2.clone(),
                b3.clone(),
            ],
            DistMode::File,
            &history,
            &HashSet::new(),
        );
        // `a.py`'s large known subtotal must not lift it past `d.py`.
        assert_eq!(
            unit_ids(&units),
            vec![
                ids(&[&b1, &b2, &b3]),
                ids(&[&d1, &d2]),
                ids(&[&a1, &a2]),
                ids(&[&c1])
            ]
        );
    }

    #[test]
    fn equal_known_totals_keep_original_order() {
        let y1 = item("y.py", "t1", &[]);
        let x1 = item("x.py", "t1", &[]);
        let x2 = item("x.py", "t2", &[]);
        let history = history(&[
            (&y1, false, Some(ms(100))),
            (&x1, false, Some(ms(50))),
            (&x2, false, Some(ms(50))),
        ]);
        let units = plan(
            vec![y1.clone(), x1.clone(), x2.clone()],
            DistMode::File,
            &history,
            &HashSet::new(),
        );
        assert_eq!(unit_ids(&units), vec![ids(&[&y1]), ids(&[&x1, &x2])]);
    }

    #[test]
    fn empty_input_yields_no_units() {
        let result = plan_work_units(Vec::new(), &[], DistMode::File, ScheduleOptions::default());
        assert!(result.units.is_empty());
        assert!(plan(Vec::new(), DistMode::Test, &HashMap::new(), &HashSet::new()).is_empty());
    }

    #[test]
    fn absent_or_empty_hints_sort_largest_first_stably() {
        let tests = vec![
            item("small.py", "t1", &[]),
            item("big.py", "t1", &[]),
            item("big.py", "t2", &[]),
            item("big.py", "t3", &[]),
            item("medium.py", "t1", &[]),
            item("medium.py", "t2", &[]),
            item("small2.py", "t1", &[]),
        ];
        let expected = vec![
            vec!["big.py::t1", "big.py::t2", "big.py::t3"],
            vec!["medium.py::t1", "medium.py::t2"],
            vec!["small.py::t1"],
            vec!["small2.py::t1"],
        ];
        let absent = plan_work_units(
            tests.clone(),
            &[],
            DistMode::File,
            ScheduleOptions::default(),
        )
        .units;
        assert_eq!(unit_ids(&absent), expected);
        let empty = plan(tests, DistMode::File, &HashMap::new(), &HashSet::new());
        assert_eq!(unit_ids(&empty), expected);
    }

    #[test]
    fn zero_duration_history_is_known() {
        let zero = item("zero.py", "t", &[]);
        let fast = item("fast.py", "t", &[]);
        let unknown = item("unknown.py", "t", &[]);
        let history = history(&[
            (&zero, false, Some(Duration::ZERO)),
            (&fast, false, Some(ms(5))),
        ]);
        let units = plan(
            vec![zero.clone(), fast.clone(), unknown.clone()],
            DistMode::Test,
            &history,
            &HashSet::new(),
        );
        assert_eq!(
            unit_ids(&units),
            vec![ids(&[&unknown]), ids(&[&fast]), ids(&[&zero])]
        );
    }

    #[test]
    fn duration_sums_saturate() {
        let a1 = item("a.py", "t1", &[]);
        let a2 = item("a.py", "t2", &[]);
        let b1 = item("b.py", "t1", &[]);
        let history = history(&[
            (&a1, false, Some(Duration::MAX)),
            (&a2, false, Some(ms(1))),
            (&b1, false, Some(Duration::MAX)),
        ]);
        let units = plan(
            vec![a1.clone(), a2.clone(), b1.clone()],
            DistMode::File,
            &history,
            &HashSet::new(),
        );
        assert_eq!(unit_ids(&units), vec![ids(&[&a1, &a2]), ids(&[&b1])]);
        let units = plan(
            vec![b1.clone(), a1.clone(), a2.clone()],
            DistMode::File,
            &history,
            &HashSet::new(),
        );
        assert_eq!(unit_ids(&units), vec![ids(&[&b1]), ids(&[&a1, &a2])]);
    }

    #[test]
    fn skip_and_todo_members_do_not_affect_rank() {
        let mut skipped = item("a.py", "skipped", &[]);
        skipped.skip = Some("reason".into());
        let mut todo = item("a.py", "todo", &[]);
        todo.todo = Some("later".into());
        let a3 = item("a.py", "t3", &[]);
        let b1 = item("b.py", "t1", &[]);
        let mut c_skipped = item("c.py", "skipped", &[]);
        c_skipped.skip = Some("reason".into());
        let history = history(&[
            (&skipped, true, Some(ms(1))),
            (&a3, false, Some(ms(10))),
            (&b1, false, Some(ms(50))),
        ]);
        let first = HashSet::from([skipped.id()]);
        let units = plan(
            vec![
                c_skipped.clone(),
                skipped.clone(),
                todo.clone(),
                a3.clone(),
                b1.clone(),
            ],
            DistMode::File,
            &history,
            &first,
        );
        assert_eq!(
            unit_ids(&units),
            vec![
                ids(&[&b1]),
                ids(&[&skipped, &todo, &a3]),
                ids(&[&c_skipped])
            ]
        );
    }

    #[test]
    fn scope_fixture_unit_is_promoted_whole() {
        let a1 = item("a.py", "t1", &[]);
        let a2 = item("a.py", "t2", &[]);
        let a3 = item("a.py", "t3", &[]);
        let b4 = item("b.py", "t4", &[]);
        let tests = vec![a1.clone(), a2.clone(), a3.clone(), b4.clone()];
        let hooks = vec![hook("setup", FixturePer::Scope, &[])];
        let passing = history(&[
            (&a1, false, Some(ms(10))),
            (&a2, false, Some(ms(5))),
            (&a3, false, Some(ms(10))),
            (&b4, false, Some(ms(500))),
        ]);
        let failing = history(&[
            (&a1, false, Some(ms(10))),
            (&a2, true, Some(ms(5))),
            (&a3, false, Some(ms(10))),
            (&b4, false, Some(ms(500))),
        ]);
        let no_priority = HashSet::new();
        let priority = HashSet::from([a3.id()]);
        let a_unit = ids(&[&a1, &a2, &a3]);

        for (history, first, a_first) in [
            (&passing, &no_priority, false),
            (&failing, &no_priority, true),
            (&passing, &priority, true),
        ] {
            let result = plan_work_units(
                tests.clone(),
                &hooks,
                DistMode::Test,
                ScheduleOptions {
                    history: Some(history),
                    first: Some(first),
                },
            );
            let kinds: Vec<_> = result.warnings.iter().map(|w| w.kind).collect();
            assert_eq!(kinds, vec![DiscoveryWarningKind::DistModeUpgrade]);
            let expected = if a_first {
                vec![a_unit.clone(), ids(&[&b4])]
            } else {
                vec![ids(&[&b4]), a_unit.clone()]
            };
            assert_eq!(unit_ids(&result.units), expected);
            for unit in &result.units {
                let hook_names: Vec<&str> = unit.hooks.iter().map(|h| h.name.as_str()).collect();
                if unit.tests[0].module_path == "a" {
                    assert_eq!(hook_names, vec!["setup"]);
                } else {
                    assert!(hook_names.is_empty());
                }
            }
        }
    }

    /// Asserts exactly one upgrade warning, the unit order, and that each
    /// unit carries exactly the hooks expected for its module.
    fn assert_fixture_plan(
        result: &PartitionResult,
        expected: &[Vec<String>],
        hooks_by_module: &[(&str, &[&str])],
    ) {
        let kinds: Vec<_> = result.warnings.iter().map(|w| w.kind).collect();
        assert_eq!(kinds, vec![DiscoveryWarningKind::DistModeUpgrade]);
        assert_eq!(unit_ids(&result.units), expected);
        for unit in &result.units {
            let module = unit.tests[0].module_path.as_str();
            let expected_hooks = hooks_by_module
                .iter()
                .find(|(m, _)| *m == module)
                .map(|(_, hooks)| *hooks);
            let hook_names: Vec<&str> = unit.hooks.iter().map(|h| h.name.as_str()).collect();
            assert_eq!(
                Some(hook_names.as_slice()),
                expected_hooks,
                "hooks for {module}"
            );
        }
    }

    /// The three fixture-upgrade variants: Test mode with a describe-scope
    /// fixture, Test mode with a file-scope fixture, and Group mode with a
    /// file-scope fixture (where `a2` sits in a different describe).
    fn fixture_variants() -> Vec<(DistMode, Vec<HookItem>, bool)> {
        let with = |fixture: HookItem| {
            vec![
                fixture,
                hook_for_module("b_hook", "b", FixturePer::Test, &[]),
                hook_for_module("c_hook", "c", FixturePer::Test, &[]),
            ]
        };
        vec![
            (
                DistMode::Test,
                with(hook("a_fixture", FixturePer::Scope, &["math"])),
                false,
            ),
            (
                DistMode::Test,
                with(hook("a_fixture", FixturePer::Scope, &[])),
                false,
            ),
            (
                DistMode::Group,
                with(hook("a_fixture", FixturePer::Scope, &[])),
                true,
            ),
        ]
    }

    const FIXTURE_HOOKS: &[(&str, &[&str])] = &[
        ("a", &["a_fixture"]),
        ("b", &["b_hook"]),
        ("c", &["c_hook"]),
    ];

    #[test]
    fn fixture_upgrade_known_ties_keep_first_discovery_position() {
        for (mode, hooks, split_a2) in fixture_variants() {
            let a1 = item("a.py", "t1", &["math"]);
            let a2 = item(
                "a.py",
                "t2",
                if split_a2 { &["strings"] } else { &["math"] },
            );
            let b1 = item("b.py", "t1", &["free"]);
            let c1 = item("c.py", "t1", &["other"]);
            let tests = vec![a1.clone(), b1.clone(), a2.clone(), c1.clone()];
            let a_unit = ids(&[&a1, &a2]);
            let run = |history: &HashMap<String, TestRunHistory>, first: &HashSet<String>| {
                plan_work_units(
                    tests.clone(),
                    &hooks,
                    mode,
                    ScheduleOptions {
                        history: Some(history),
                        first: Some(first),
                    },
                )
            };
            let none = HashSet::new();

            let tied = history(&[
                (&a1, false, Some(ms(4))),
                (&a2, false, Some(ms(6))),
                (&b1, false, Some(ms(10))),
                (&c1, false, Some(ms(10))),
            ]);
            assert_fixture_plan(
                &run(&tied, &none),
                &[a_unit.clone(), ids(&[&b1]), ids(&[&c1])],
                FIXTURE_HOOKS,
            );

            let b_longer = history(&[
                (&a1, false, Some(ms(4))),
                (&a2, false, Some(ms(6))),
                (&b1, false, Some(ms(11))),
                (&c1, false, Some(ms(10))),
            ]);
            assert_fixture_plan(
                &run(&b_longer, &none),
                &[ids(&[&b1]), a_unit.clone(), ids(&[&c1])],
                FIXTURE_HOOKS,
            );

            let b_failing = history(&[
                (&a1, false, Some(ms(4))),
                (&a2, false, Some(ms(6))),
                (&b1, true, Some(ms(10))),
                (&c1, false, Some(ms(10))),
            ]);
            assert_fixture_plan(
                &run(&b_failing, &none),
                &[ids(&[&b1]), a_unit.clone(), ids(&[&c1])],
                FIXTURE_HOOKS,
            );
            assert_fixture_plan(
                &run(&b_failing, &HashSet::from([c1.id()])),
                &[ids(&[&c1]), ids(&[&b1]), a_unit.clone()],
                FIXTURE_HOOKS,
            );

            let b_unknown = history(&[
                (&a1, false, Some(ms(4))),
                (&a2, false, Some(ms(6))),
                (&c1, false, Some(ms(10))),
            ]);
            assert_fixture_plan(
                &run(&b_unknown, &none),
                &[ids(&[&b1]), a_unit.clone(), ids(&[&c1])],
                FIXTURE_HOOKS,
            );
        }
    }

    #[test]
    fn fixture_upgrade_cold_ties_keep_discovery_order() {
        for (mode, hooks, _) in fixture_variants() {
            let a1 = item("a.py", "t1", &["math"]);
            let b1 = item("b.py", "t1", &["free"]);
            let c1 = item("c.py", "t1", &["other"]);
            let tests = vec![a1.clone(), b1.clone(), c1.clone()];
            let expected = [ids(&[&a1]), ids(&[&b1]), ids(&[&c1])];

            let absent = plan_work_units(tests.clone(), &hooks, mode, ScheduleOptions::default());
            assert_fixture_plan(&absent, &expected, FIXTURE_HOOKS);

            let empty_history = HashMap::new();
            let empty_first = HashSet::new();
            let empty = plan_work_units(
                tests,
                &hooks,
                mode,
                ScheduleOptions {
                    history: Some(&empty_history),
                    first: Some(&empty_first),
                },
            );
            assert_fixture_plan(&empty, &expected, FIXTURE_HOOKS);
        }
    }

    #[test]
    fn mixed_fixture_constraints_keep_grouping_and_discovery_ties() {
        let a1 = item("a.py", "t1", &["math"]);
        let b1 = item("b.py", "t1", &["free"]);
        let c1 = item("c.py", "t1", &["left"]);
        let a2 = item("a.py", "t2", &["math"]);
        let c2 = item("c.py", "t2", &["right"]);
        let hooks = vec![
            hook("a_fixture", FixturePer::Scope, &["math"]),
            hook_for_module("b_hook", "b", FixturePer::Test, &[]),
            hook_for_module("c_fixture", "c", FixturePer::Scope, &[]),
        ];
        let history = history(&[
            (&a1, false, Some(ms(4))),
            (&a2, false, Some(ms(6))),
            (&b1, false, Some(ms(10))),
            (&c1, false, Some(ms(5))),
            (&c2, false, Some(ms(5))),
        ]);
        let first = HashSet::new();
        let result = plan_work_units(
            vec![a1.clone(), b1.clone(), c1.clone(), a2.clone(), c2.clone()],
            &hooks,
            DistMode::Test,
            ScheduleOptions {
                history: Some(&history),
                first: Some(&first),
            },
        );
        assert_fixture_plan(
            &result,
            &[ids(&[&a1, &a2]), ids(&[&b1]), ids(&[&c1, &c2])],
            &[
                ("a", &["a_fixture"]),
                ("b", &["b_hook"]),
                ("c", &["c_fixture"]),
            ],
        );
    }
}
