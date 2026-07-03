use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use log::debug;
use notify::RecommendedWatcher;
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};
use tokio::sync::mpsc;
use tryke_discovery::build_change_set_ignore;

const DEBOUNCE_DELAY: Duration = Duration::from_millis(50);

#[derive(Debug, PartialEq, Eq)]
pub struct FileChangeBatch {
    pub paths: Vec<PathBuf>,
}

enum WatchEvent {
    Paths(Vec<PathBuf>),
    Error(String),
}

/// Watches a project for meaningful Python file changes.
///
/// This type owns the underlying OS watcher and normalizes its raw events into
/// coalesced, deduplicated batches suitable for async consumers.
pub struct FileWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
    rx: mpsc::UnboundedReceiver<WatchEvent>,
    change_filter: ChangeFilter,
}

impl FileWatcher {
    /// Starts a recursive watcher for Python files under `root`.
    ///
    /// # Errors
    /// Returns an error if the watcher cannot be created or subscribed to
    /// `root`.
    pub fn spawn(root: &Path, excludes: &[String]) -> anyhow::Result<Self> {
        let gitignore = build_change_set_ignore(root, excludes);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut debouncer = new_debouncer(DEBOUNCE_DELAY, move |result: DebounceEventResult| {
            let event = match result {
                Ok(events) => {
                    let paths = events
                        .into_iter()
                        .filter(|event| {
                            event
                                .path
                                .extension()
                                .is_some_and(|extension| extension == "py")
                        })
                        .filter(|event| {
                            !gitignore
                                .matched_path_or_any_parents(&event.path, false)
                                .is_ignore()
                        })
                        .map(|event| event.path)
                        .collect::<Vec<_>>();
                    WatchEvent::Paths(paths)
                }
                Err(error) => WatchEvent::Error(format!("{error:?}")),
            };
            let _ = tx.send(event);
        })?;
        debouncer
            .watcher()
            .watch(root, notify::RecursiveMode::Recursive)?;

        Ok(Self {
            _debouncer: debouncer,
            rx,
            change_filter: ChangeFilter::default(),
        })
    }

    /// Returns the next non-empty batch of meaningful file changes.
    ///
    /// Events already queued together are coalesced before content signatures
    /// are checked, preventing a single editor save from producing duplicate
    /// consumer work.
    ///
    /// # Errors
    /// Returns an error reported by the underlying filesystem watcher.
    pub async fn next_batch(&mut self) -> anyhow::Result<Option<FileChangeBatch>> {
        loop {
            let Some(first) = self.rx.recv().await else {
                return Ok(None);
            };
            let mut paths = match first {
                WatchEvent::Paths(paths) => paths,
                WatchEvent::Error(error) => anyhow::bail!("filesystem watcher failed: {error}"),
            };

            while let Ok(event) = self.rx.try_recv() {
                match event {
                    WatchEvent::Paths(more) => paths.extend(more),
                    WatchEvent::Error(error) => {
                        anyhow::bail!("filesystem watcher failed: {error}");
                    }
                }
            }

            paths.sort();
            paths.dedup();
            let paths = self.change_filter.filter(&paths);
            if paths.is_empty() {
                debug!("file watcher: change batch had no meaningful changes");
                continue;
            }
            return Ok(Some(FileChangeBatch { paths }));
        }
    }

    /// Discards batches that are already queued.
    pub fn discard_pending(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileSignature {
    mtime_nanos: i128,
    size: u64,
}

impl FileSignature {
    fn from_path(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata.modified().ok()?;
        let mtime_nanos = match modified.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(duration) => i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX),
            Err(error) => -i128::try_from(error.duration().as_nanos()).unwrap_or(i128::MAX),
        };
        Some(Self {
            mtime_nanos,
            size: metadata.len(),
        })
    }
}

#[derive(Debug, Default)]
struct ChangeFilter {
    last_seen: HashMap<PathBuf, Option<FileSignature>>,
}

impl ChangeFilter {
    fn filter(&mut self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut changed = Vec::with_capacity(paths.len());
        for path in paths {
            let current = FileSignature::from_path(path);
            if self
                .last_seen
                .get(path)
                .is_none_or(|prior| *prior != current)
            {
                self.last_seen.insert(path.clone(), current);
                changed.push(path.clone());
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use super::*;

    #[test]
    fn change_filter_drops_unchanged_paths() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("test_example.py");
        fs::write(&path, "x = 1").expect("write");
        let mut filter = ChangeFilter::default();

        assert_eq!(
            filter.filter(std::slice::from_ref(&path)),
            vec![path.clone()]
        );
        assert!(filter.filter(&[path]).is_empty());
    }

    #[test]
    fn change_filter_reports_deletions() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("test_example.py");
        fs::write(&path, "x = 1").expect("write");
        let mut filter = ChangeFilter::default();
        let _ = filter.filter(std::slice::from_ref(&path));

        fs::remove_file(&path).expect("remove");

        assert_eq!(filter.filter(std::slice::from_ref(&path)), vec![path]);
    }

    #[test]
    fn change_filter_reports_content_changes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("test_example.py");
        fs::write(&path, "x = 1").expect("write");
        let mut filter = ChangeFilter::default();
        let _ = filter.filter(std::slice::from_ref(&path));

        fs::write(&path, "x = 222").expect("update");

        assert_eq!(filter.filter(std::slice::from_ref(&path)), vec![path]);
    }

    #[test]
    fn change_filter_reports_first_observation_of_missing_path() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("deleted.py");
        let mut filter = ChangeFilter::default();

        assert_eq!(filter.filter(std::slice::from_ref(&path)), vec![path]);
    }

    #[tokio::test]
    async fn watcher_emits_python_file_changes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("test_example.py");
        fs::write(&path, "x = 1").expect("write");
        let mut watcher = FileWatcher::spawn(directory.path(), &[]).expect("spawn watcher");
        tokio::time::sleep(Duration::from_millis(500)).await;

        let batch = tokio::time::timeout(Duration::from_secs(5), async {
            for value in 2..=10 {
                fs::write(&path, format!("x = {value}")).expect("update");
                if let Ok(batch) =
                    tokio::time::timeout(Duration::from_millis(500), watcher.next_batch()).await
                {
                    return batch;
                }
            }
            panic!("watcher did not observe a file change");
        })
        .await
        .expect("watcher timeout")
        .expect("watcher error")
        .expect("watcher closed");
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        assert!(
            batch
                .paths
                .iter()
                .any(|changed| changed == &path || changed == &canonical),
            "expected {} in {:?}",
            path.display(),
            batch.paths,
        );
    }

    #[tokio::test]
    async fn watcher_ignores_non_python_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut watcher = FileWatcher::spawn(directory.path(), &[]).expect("spawn watcher");
        tokio::time::sleep(Duration::from_millis(500)).await;

        fs::write(directory.path().join("notes.txt"), "notes").expect("write text file");

        assert!(
            tokio::time::timeout(Duration::from_millis(500), watcher.next_batch())
                .await
                .is_err(),
            "non-Python changes must not produce a batch",
        );
    }
}
