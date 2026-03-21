use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::Result;
use tryke_discovery::Discoverer;
use tryke_reporter::Reporter;
use tryke_runner::{DistMode, WorkerPool, check_python_version, resolve_python};
use tryke_types::{DiscoveryWarning, DiscoveryWarningKind, filter::TestFilter};

use crate::execution::{report_cycle, worker_pool_size};

fn clear_if_tty() {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        let _ = clearscreen::clear();
    }
}

fn emit_dynamic_import_warnings(reporter: &mut dyn Reporter, discoverer: &Discoverer) {
    for path in discoverer.dynamic_import_files() {
        let message = format!(
            "{} — dynamic imports found; will always re-run and may serve stale module state in watch mode",
            path.display()
        );
        reporter.on_discovery_warning(&DiscoveryWarning {
            file_path: path,
            kind: DiscoveryWarningKind::DynamicImports,
            message,
        });
    }
}

pub async fn run_watch(
    reporter: &mut dyn Reporter,
    root: Option<&Path>,
    excludes: &[String],
    test_filter: &TestFilter,
    maxfail: Option<usize>,
    workers: Option<usize>,
    dist: DistMode,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new_with_excludes(root, excludes);

    let python = resolve_python(root);
    check_python_version(&python, root)?;
    let pool_size = workers.unwrap_or_else(worker_pool_size);
    let pool = WorkerPool::new(pool_size, &python, root);
    pool.warm().await;

    clear_if_tty();
    let disc_start = Instant::now();
    let tests = test_filter.apply(discoverer.rediscover());
    let disc_dur = Some(disc_start.elapsed());
    emit_dynamic_import_warnings(reporter, &discoverer);
    report_cycle(reporter, tests, &pool, maxfail, dist, disc_dur, None).await?;

    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let _debouncer = tryke_server::watcher::spawn_watcher(root, excludes, tx)?;

    for paths in &rx {
        let modules = discoverer.affected_modules(&paths);
        if !modules.is_empty() {
            pool.reload(modules).await;
        }
        discoverer.rediscover_changed(&paths);
        clear_if_tty();
        let disc_start = Instant::now();
        let tests = test_filter.apply(discoverer.tests_for_changed(&paths));
        let disc_dur = Some(disc_start.elapsed());
        emit_dynamic_import_warnings(reporter, &discoverer);
        report_cycle(reporter, tests, &pool, maxfail, dist, disc_dur, None).await?;
    }

    pool.shutdown();
    Ok(())
}
