use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use log::debug;
use notify::RecommendedWatcher;
use notify_debouncer_mini::{DebounceEventResult, DebouncedEvent, Debouncer, new_debouncer};
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

struct ChangeQueue {
    rx: mpsc::UnboundedReceiver<WatchEvent>,
    change_filter: ChangeFilter,
}

impl ChangeQueue {
    fn channel() -> (mpsc::UnboundedSender<WatchEvent>, Self) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            tx,
            Self {
                rx,
                change_filter: ChangeFilter::default(),
            },
        )
    }

    async fn next_batch(&mut self) -> anyhow::Result<Option<FileChangeBatch>> {
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

    fn discard_pending(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }
}

fn relevant_paths(events: Vec<DebouncedEvent>, is_ignored: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    events
        .into_iter()
        .filter(|event| {
            event
                .path
                .extension()
                .is_some_and(|extension| extension == "py")
        })
        .filter(|event| !is_ignored(&event.path))
        .map(|event| event.path)
        .collect()
}

/// Watches a project for meaningful Python file changes.
///
/// This type owns the underlying OS watcher and normalizes its raw events into
/// coalesced, deduplicated batches suitable for async consumers.
pub struct FileWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
    changes: ChangeQueue,
}

impl FileWatcher {
    /// Starts a recursive watcher for Python files under `root`.
    ///
    /// # Errors
    /// Returns an error if the watcher cannot be created or subscribed to
    /// `root`.
    pub fn spawn(root: &Path, excludes: &[String]) -> anyhow::Result<Self> {
        let gitignore = build_change_set_ignore(root, excludes);
        let (tx, changes) = ChangeQueue::channel();
        let mut debouncer = new_debouncer(DEBOUNCE_DELAY, move |result: DebounceEventResult| {
            let event = match result {
                Ok(events) => WatchEvent::Paths(relevant_paths(events, |path| {
                    gitignore
                        .matched_path_or_any_parents(path, false)
                        .is_ignore()
                })),
                Err(error) => WatchEvent::Error(format!("{error:?}")),
            };
            let _ = tx.send(event);
        })?;
        debouncer
            .watcher()
            .watch(root, notify::RecursiveMode::Recursive)?;

        Ok(Self {
            _debouncer: debouncer,
            changes,
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
        self.changes.next_batch().await
    }

    /// Discards batches that are already queued.
    pub fn discard_pending(&mut self) {
        self.changes.discard_pending();
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
    use std::fs;

    use super::*;
    use notify_debouncer_mini::DebouncedEventKind;

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

    #[test]
    fn relevant_paths_filters_non_python_and_ignored_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        let python = directory.path().join("test_example.py");
        let ignored = directory.path().join("ignored.py");
        let text = directory.path().join("notes.txt");
        let event = |path| DebouncedEvent {
            path,
            kind: DebouncedEventKind::Any,
        };

        let paths = relevant_paths(
            vec![event(python.clone()), event(ignored.clone()), event(text)],
            |path| path == ignored,
        );

        assert_eq!(paths, vec![python]);
    }

    #[tokio::test]
    async fn change_queue_coalesces_manually_triggered_events() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("test_example.py");
        fs::write(&path, "x = 1").expect("write");
        let (events, mut changes) = ChangeQueue::channel();

        events
            .send(WatchEvent::Paths(vec![path.clone(), path.clone()]))
            .expect("send initial event");
        let batch = changes
            .next_batch()
            .await
            .expect("queue error")
            .expect("queue closed");
        assert_eq!(batch.paths, vec![path.clone()]);

        fs::write(&path, "x = 222").expect("update");
        events
            .send(WatchEvent::Paths(Vec::new()))
            .expect("send empty event");
        events
            .send(WatchEvent::Paths(vec![path.clone()]))
            .expect("send changed event");
        let batch = changes
            .next_batch()
            .await
            .expect("queue error")
            .expect("queue closed");
        assert_eq!(batch.paths, vec![path]);
    }
}
