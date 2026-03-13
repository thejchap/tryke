use std::time::{Duration, Instant};

use anyhow::Result;
use tokio_stream::StreamExt;
use tryke_reporter::Reporter;
use tryke_runner::{WorkerPool, check_python_version, resolve_python};
use tryke_types::{ChangedSelectionSummary, RunSummary, TestOutcome};

pub fn worker_pool_size() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
}

pub async fn run_tests(
    reporter: &mut dyn Reporter,
    root: &std::path::Path,
    tests: Vec<tryke_types::TestItem>,
    maxfail: Option<usize>,
    workers: Option<usize>,
    discovery_duration: Option<Duration>,
    changed_selection: Option<ChangedSelectionSummary>,
) -> Result<()> {
    let python = resolve_python(root);
    check_python_version(&python, root)?;
    let pool_size = workers.unwrap_or_else(|| tests.len().min(worker_pool_size()));
    let pool = WorkerPool::new(pool_size, &python, root);
    pool.warm().await;
    report_cycle(
        reporter,
        tests,
        &pool,
        maxfail,
        discovery_duration,
        changed_selection,
    )
    .await?;
    pool.shutdown();
    Ok(())
}

pub async fn report_cycle(
    reporter: &mut dyn Reporter,
    tests: Vec<tryke_types::TestItem>,
    pool: &WorkerPool,
    maxfail: Option<usize>,
    discovery_duration: Option<Duration>,
    changed_selection: Option<ChangedSelectionSummary>,
) -> Result<()> {
    use std::collections::HashSet;

    let file_count = tests
        .iter()
        .filter_map(|t| t.file_path.as_ref())
        .collect::<HashSet<_>>()
        .len();

    let start_time = chrono::Local::now().format("%H:%M:%S").to_string();

    let start = Instant::now();
    reporter.on_run_start(&tests);

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut xfailed = 0usize;
    let mut todo = 0usize;

    // Short-circuit skip/todo tests — no worker needed
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
        reporter.on_test_complete(&result);
    }

    let mut stream = pool.run(run_tests);
    while let Some(result) = stream.next().await {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } | TestOutcome::XPassed => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
            TestOutcome::Error { .. } => errors += 1,
            TestOutcome::XFailed { .. } => xfailed += 1,
            TestOutcome::Todo { .. } => todo += 1,
        }
        reporter.on_test_complete(&result);
        if let Some(max) = maxfail
            && failed >= max
        {
            break;
        }
    }

    reporter.on_run_complete(&RunSummary {
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
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{env, path::PathBuf};

    use tryke_discovery::Discoverer;
    use tryke_reporter::{DotReporter, JSONReporter, JUnitReporter, TextReporter};
    use tryke_types::TestOutcome;

    use super::*;
    use crate::discovery::{discover_tests, resolved_excludes};

    fn test_python_bin() -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        tryke_runner::resolve_python(&root)
    }

    fn cwd() -> PathBuf {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    async fn run_cycle(
        reporter: &mut dyn Reporter,
        discoverer: &mut Discoverer,
        pool: &WorkerPool,
    ) -> anyhow::Result<()> {
        report_cycle(reporter, discoverer.rediscover(), pool, None, None, None).await
    }

    #[tokio::test]
    async fn test_command_text() {
        let mut reporter = TextReporter::with_writer(Vec::new());
        let root = cwd();
        let excludes = resolved_excludes(&root, &[], &[]);
        let tests = discover_tests(&root, false, None, &excludes).tests;
        assert!(
            run_tests(&mut reporter, &root, tests, None, None, None, None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_command_json() {
        let mut reporter = JSONReporter::with_writer(Vec::new());
        let root = cwd();
        let excludes = resolved_excludes(&root, &[], &[]);
        let tests = discover_tests(&root, false, None, &excludes).tests;
        assert!(
            run_tests(&mut reporter, &root, tests, None, None, None, None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_command_dot() {
        let mut reporter = DotReporter::with_writer(Vec::new());
        let root = cwd();
        let excludes = resolved_excludes(&root, &[], &[]);
        let tests = discover_tests(&root, false, None, &excludes).tests;
        assert!(
            run_tests(&mut reporter, &root, tests, None, None, None, None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_command_junit() {
        let mut reporter = JUnitReporter::with_writer(Vec::new());
        let root = cwd();
        let excludes = resolved_excludes(&root, &[], &[]);
        let tests = discover_tests(&root, false, None, &excludes).tests;
        assert!(
            run_tests(&mut reporter, &root, tests, None, None, None, None)
                .await
                .is_ok()
        );
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
        // non-git directory → git_changed_files returns None → discover_tests runs all (0 here)
        let tests = discover_tests(dir.path(), true, None, &[]).tests;
        assert!(
            run_tests(&mut reporter, dir.path(), tests, None, None, None, None)
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
        let python = test_python_bin();
        tryke_runner::check_python_version(&python, &workspace_root)
            .expect("Python version check (from pyproject.toml requires-python)");

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
        let mut results: Vec<_> = pool.run(tests).collect().await;
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
}
