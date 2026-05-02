use std::time::{Duration, Instant};

use anyhow::Result;
use tokio_stream::StreamExt;
use tryke_reporter::Reporter;
use tryke_runner::{DistMode, WorkerPool, partition_with_hooks};
use tryke_types::{ChangedSelectionSummary, HookItem, RunSummary, TestOutcome};

pub fn worker_pool_size() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
}

#[expect(clippy::too_many_arguments)]
pub async fn run_tests(
    reporter: &mut dyn Reporter,
    root: &std::path::Path,
    python: &str,
    tests: Vec<tryke_types::TestItem>,
    hooks: &[HookItem],
    maxfail: Option<usize>,
    workers: Option<usize>,
    dist: DistMode,
    discovery_duration: Option<Duration>,
    changed_selection: Option<ChangedSelectionSummary>,
) -> Result<RunSummary> {
    let pool_size = workers.unwrap_or_else(|| tests.len().min(worker_pool_size()));
    let pool = WorkerPool::new(pool_size, python, root);
    pool.warm().await;
    let summary = report_cycle(
        reporter,
        tests,
        hooks,
        &pool,
        maxfail,
        dist,
        discovery_duration,
        changed_selection,
    )
    .await?;
    pool.shutdown();
    Ok(summary)
}

fn flush_buffer(
    file: &Option<std::path::PathBuf>,
    buffers: &mut std::collections::HashMap<
        Option<std::path::PathBuf>,
        Vec<(usize, tryke_types::TestResult)>,
    >,
    reporter: &mut dyn Reporter,
) {
    if let Some(mut buf) = buffers.remove(file) {
        buf.sort_by_key(|(idx, _)| *idx);
        for (_, result) in buf {
            reporter.on_test_complete(&result);
        }
    }
}

#[expect(clippy::too_many_arguments)]
pub async fn report_cycle(
    reporter: &mut dyn Reporter,
    tests: Vec<tryke_types::TestItem>,
    hooks: &[HookItem],
    pool: &WorkerPool,
    maxfail: Option<usize>,
    dist: DistMode,
    discovery_duration: Option<Duration>,
    changed_selection: Option<ChangedSelectionSummary>,
) -> Result<RunSummary> {
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    let file_count = tests
        .iter()
        .filter_map(|t| t.file_path.as_ref())
        .collect::<HashSet<_>>()
        .len();

    let start_time = chrono::Local::now().format("%H:%M:%S").to_string();

    // build discovery-order index and per-file expected counts
    // before partitioning so shortcircuit tests are included
    let discovery_order: HashMap<String, usize> =
        tests.iter().enumerate().map(|(i, t)| (t.id(), i)).collect();

    let mut expected_per_file: HashMap<Option<PathBuf>, usize> = HashMap::new();
    for t in &tests {
        *expected_per_file.entry(t.file_path.clone()).or_default() += 1;
    }

    let start = Instant::now();
    reporter.on_run_start(&tests);

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut xfailed = 0usize;
    let mut todo = 0usize;

    type FileBuffer = Vec<(usize, tryke_types::TestResult)>;
    let mut buffers: HashMap<Option<PathBuf>, FileBuffer> = HashMap::new();

    // short-circuit skip/todo tests — buffer instead of reporting eagerly
    let (run_tests, shortcircuit): (Vec<_>, Vec<_>) = tests
        .into_iter()
        .partition(|t| t.skip.is_none() && t.todo.is_none());

    for t in shortcircuit {
        let outcome = if t.todo.is_some() {
            todo += 1;
            TestOutcome::Todo {
                description: t.todo.clone(),
            }
        } else {
            skipped += 1;
            TestOutcome::Skipped {
                reason: t.skip.clone(),
            }
        };
        let result = tryke_types::TestResult {
            test: t,
            outcome,
            duration: std::time::Duration::ZERO,
            stdout: String::new(),
            stderr: String::new(),
        };
        let idx = discovery_order
            .get(&result.test.id())
            .copied()
            .unwrap_or(usize::MAX);
        let file = result.test.file_path.clone();
        buffers.entry(file).or_default().push((idx, result));
    }

    let mut hit_maxfail = false;
    let partition = partition_with_hooks(run_tests, hooks, dist);
    for w in &partition.warnings {
        eprintln!("warning: {w}");
    }
    let mut stream = pool.run(partition.units);
    while let Some(result) = stream.next().await {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } | TestOutcome::XPassed => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
            TestOutcome::Error { .. } => errors += 1,
            TestOutcome::XFailed { .. } => xfailed += 1,
            TestOutcome::Todo { .. } => todo += 1,
        }

        let idx = discovery_order
            .get(&result.test.id())
            .copied()
            .unwrap_or(usize::MAX);
        let file = result.test.file_path.clone();
        buffers.entry(file.clone()).or_default().push((idx, result));

        // flush if this file's buffer is complete
        if let Some(&expected) = expected_per_file.get(&file)
            && buffers.get(&file).is_some_and(|b| b.len() >= expected)
        {
            flush_buffer(&file, &mut buffers, reporter);
        }

        if let Some(max) = maxfail
            && failed >= max
        {
            hit_maxfail = true;
            break;
        }
    }

    // flush any remaining buffered files (partial files from maxfail, or edge cases)
    if hit_maxfail || !buffers.is_empty() {
        let mut remaining: Vec<(usize, Option<PathBuf>)> = buffers
            .iter()
            .map(|(file, buf)| {
                let min_idx = buf.iter().map(|(idx, _)| *idx).min().unwrap_or(usize::MAX);
                (min_idx, file.clone())
            })
            .collect();
        remaining.sort_by_key(|(idx, _)| *idx);
        for (_, file) in remaining {
            flush_buffer(&file, &mut buffers, reporter);
        }
    }

    let summary = RunSummary {
        passed,
        failed,
        skipped,
        errors,
        xfailed,
        todo,
        duration: discovery_duration.unwrap_or_default() + start.elapsed(),
        discovery_duration,
        test_duration: Some(start.elapsed()),
        file_count,
        start_time: Some(start_time),
        changed_selection,
    };
    reporter.on_run_complete(&summary);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tryke_discovery::Discoverer;
    use tryke_reporter::{
        DotReporter, JSONReporter, JUnitReporter, NextReporter, SugarReporter, TextReporter,
    };
    use tryke_types::TestOutcome;

    use super::*;
    use crate::discovery::{discover_tests, resolved_excludes};

    /// Use the workspace's venv interpreter if present, otherwise fall back
    /// to a bare-name lookup on `PATH`. Tests need a Python that satisfies
    /// the project's `requires-python`; locally the uv-managed venv
    /// covers that, and CI runs nextest under `uv run` so the venv is
    /// already on `PATH`. Venv layout and the bare fallback both differ
    /// per OS — Windows uses `Scripts/python.exe` + `python`, Unix uses
    /// `bin/python3` + `python3` — matching `tryke_config::default_python`.
    fn test_python_bin() -> String {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (venv, fallback) = if cfg!(windows) {
            (workspace.join(".venv/Scripts/python.exe"), "python")
        } else {
            (workspace.join(".venv/bin/python3"), "python3")
        };
        if venv.exists() {
            venv.to_string_lossy().into_owned()
        } else {
            fallback.to_owned()
        }
    }

    async fn run_cycle(
        reporter: &mut dyn Reporter,
        discoverer: &mut Discoverer,
        pool: &WorkerPool,
    ) -> anyhow::Result<RunSummary> {
        report_cycle(
            reporter,
            discoverer.rediscover(),
            &[],
            pool,
            None,
            DistMode::Test,
            None,
            None,
        )
        .await
    }

    /// Smoke-test a reporter against the full `run_tests` pipeline using an
    /// empty project. Exercises pool init/teardown and the reporter's
    /// run_start/run_summary callbacks without doing real work. Snapshot
    /// tests in `tests/snapshots.rs` cover per-test rendering.
    async fn smoke_run_tests(reporter: &mut dyn Reporter) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let excludes = resolved_excludes(dir.path(), &[], &[]);
        let tests = discover_tests(dir.path(), false, None, &excludes).tests;
        let _ = run_tests(
            reporter,
            dir.path(),
            &test_python_bin(),
            tests,
            &[],
            None,
            None,
            DistMode::Test,
            None,
            None,
        )
        .await;
    }

    #[tokio::test]
    async fn test_command_text() {
        let mut reporter = TextReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await;
    }

    #[tokio::test]
    async fn test_command_json() {
        let mut reporter = JSONReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await;
    }

    #[tokio::test]
    async fn test_command_dot() {
        let mut reporter = DotReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await;
    }

    #[tokio::test]
    async fn test_command_junit() {
        let mut reporter = JUnitReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await;
    }

    #[tokio::test]
    async fn test_command_next() {
        let mut reporter = NextReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await;
    }

    #[tokio::test]
    async fn test_command_sugar() {
        let mut reporter = SugarReporter::with_writer(Vec::new());
        smoke_run_tests(&mut reporter).await;
    }

    #[tokio::test]
    async fn run_cycle_runs_without_error() {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_x.py"),
            "from tryke import test\n\n@test\ndef test_x(): pass\n",
        )
        .expect("write test file");
        let mut discoverer = Discoverer::new(dir.path());
        let mut reporter = TextReporter::new();
        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_dir],
        );
        assert!(
            run_cycle(&mut reporter, &mut discoverer, &pool)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn run_cycle_with_json_reporter() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let mut discoverer = Discoverer::new(dir.path());
        let mut reporter = JSONReporter::with_writer(Vec::new());
        let pool = WorkerPool::new(1, &test_python_bin(), dir.path());
        assert!(
            run_cycle(&mut reporter, &mut discoverer, &pool)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn run_changed_test_without_git_runs_all() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        let mut reporter = TextReporter::new();
        // Non-git directory → git_changed_files returns None → discover_tests runs all (0 here)
        let tests = discover_tests(dir.path(), true, None, &[]).tests;
        assert!(
            run_tests(
                &mut reporter,
                dir.path(),
                &test_python_bin(),
                tests,
                &[],
                None,
                None,
                DistMode::Test,
                None,
                None
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn integration_python_worker_runs_tests() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        let python_dir = workspace_root.join("python");

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_example.py"),
            "\
from tryke import test, expect

@test
def test_passing():
    expect(1 + 1).to_equal(2)

@test
def test_failing():
    expect(1 + 1).to_equal(3)
",
        )
        .expect("write test file");

        let tests = discover_tests(dir.path(), false, None, &[]).tests;
        assert_eq!(tests.len(), 2);

        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_dir],
        );
        pool.warm().await;
        let units = partition_with_hooks(tests, &[], DistMode::Test).units;
        let mut results: Vec<_> = pool.run(units).collect().await;
        results.sort_by(|a, b| a.test.name.cmp(&b.test.name));

        assert_eq!(results.len(), 2);
        assert!(
            matches!(results[0].outcome, TestOutcome::Failed { .. }),
            "test_failing should fail, got {:?}",
            results[0].outcome
        );
        assert!(
            matches!(results[1].outcome, TestOutcome::Passed),
            "test_passing should pass, got {:?}",
            results[1].outcome
        );
        for r in &results {
            assert!(
                !matches!(r.outcome, TestOutcome::Error { .. }),
                "unexpected worker error: {:?}",
                r.outcome
            );
        }

        pool.shutdown();
    }

    #[tokio::test]
    async fn report_cycle_returns_ok_when_all_pass() {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_pass.py"),
            "from tryke import test, expect\n\n@test\ndef test_ok():\n    expect(1 + 1).to_equal(2)\n",
        )
        .expect("write test file");
        let tests = discover_tests(dir.path(), false, None, &[]).tests;
        let mut reporter = TextReporter::with_writer(Vec::new());
        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_dir],
        );
        let result = report_cycle(
            &mut reporter,
            tests,
            &[],
            &pool,
            None,
            DistMode::Test,
            None,
            None,
        )
        .await;
        assert!(
            result.is_ok(),
            "expected Ok when all tests pass, got {result:?}"
        );
    }

    #[tokio::test]
    async fn report_cycle_summary_reports_failures() {
        let python_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python")
            .canonicalize()
            .expect("python/ dir must exist");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_fail.py"),
            "from tryke import test, expect\n\n@test\ndef test_bad():\n    expect(1 + 1).to_equal(3)\n",
        )
        .expect("write test file");
        let tests = discover_tests(dir.path(), false, None, &[]).tests;
        let mut reporter = TextReporter::with_writer(Vec::new());
        let pool = WorkerPool::with_python_path(
            1,
            &test_python_bin(),
            dir.path(),
            &[dir.path().to_path_buf(), python_dir],
        );
        let summary = report_cycle(
            &mut reporter,
            tests,
            &[],
            &pool,
            None,
            DistMode::Test,
            None,
            None,
        )
        .await
        .expect("report_cycle should not error on test failures");
        assert_eq!(summary.failed, 1, "expected one failed test");
        assert_eq!(summary.passed, 0);
    }
}
