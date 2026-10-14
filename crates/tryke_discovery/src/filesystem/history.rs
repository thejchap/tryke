//! Persistent last-observed outcome/duration per test, used to order
//! work units (previous failures first, then unknown timings, then the
//! longest estimated work). Stored beside the discovery cache.
//!
//! History is advisory: a missing, unreadable, corrupt, wrong-version, or
//! wrong-root file loads empty, and save failures never affect a run.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use log::{debug, trace};
use serde::{Deserialize, Serialize};
use tryke_types::{TestOutcome, TestResult, TestRunHistory};

use super::cache::{GitignoreConfig, write_cache};

/// Name of the scheduling history file within the cache directory. The
/// stem is also reused (with a `.tmp` extension) for the atomic write.
pub(super) const SCHEDULING_CACHE_FILE_NAME: &str = "scheduling-v1.bin";

/// Bumped whenever the history schema changes. A mismatch loads empty.
const SCHEDULING_HISTORY_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    root: PathBuf,
    entries: HashMap<String, TestRunHistory>,
}

impl Default for HistoryFile {
    fn default() -> Self {
        Self {
            version: SCHEDULING_HISTORY_VERSION,
            root: PathBuf::new(),
            entries: HashMap::new(),
        }
    }
}

/// Last-observed per-test scheduling history keyed by `TestItem::id()`.
///
/// `Default` is an empty, memory-only store: recording works, saving is a
/// no-op.
#[derive(Debug, Default)]
pub struct SchedulingHistory {
    file: HistoryFile,
    /// Backing file. `None` disables I/O.
    path: Option<PathBuf>,
    gitignore: Option<GitignoreConfig>,
    /// Set when an entry changed since the last successful save.
    dirty: bool,
}

impl SchedulingHistory {
    pub(super) fn load(
        root: &Path,
        path: Option<PathBuf>,
        gitignore: Option<GitignoreConfig>,
    ) -> Self {
        let entries =
            path.as_deref()
                .map_or_else(HashMap::new, |path| match Self::try_load(root, path) {
                    Ok(entries) => {
                        debug!(
                            "Scheduling history loaded {} entries from {}",
                            entries.len(),
                            path.display()
                        );
                        entries
                    }
                    Err(err) => {
                        trace!("Scheduling history load failed ({err}): starting empty");
                        HashMap::new()
                    }
                });
        Self {
            file: HistoryFile {
                version: SCHEDULING_HISTORY_VERSION,
                root: root.to_path_buf(),
                entries,
            },
            path,
            gitignore,
            dirty: false,
        }
    }

    fn try_load(root: &Path, path: &Path) -> io::Result<HashMap<String, TestRunHistory>> {
        let bytes = fs::read(path)?;
        let file: HistoryFile = rmp_serde::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if file.version != SCHEDULING_HISTORY_VERSION {
            debug!(
                "Scheduling history version mismatch ({} vs {SCHEDULING_HISTORY_VERSION}): discarding",
                file.version
            );
            return Ok(HashMap::new());
        }
        if file.root != root {
            debug!(
                "Scheduling history root mismatch ({} vs {}): discarding",
                file.root.display(),
                root.display()
            );
            return Ok(HashMap::new());
        }
        Ok(file.entries)
    }

    #[must_use]
    pub fn entries(&self) -> &HashMap<String, TestRunHistory> {
        &self.file.entries
    }

    /// Record the latest eligible outcome for `result.test`.
    ///
    /// Skips and todos leave the entry unchanged. Zero-duration errors are
    /// synthetic (worker spawn/RPC failures), so they keep any previous
    /// duration rather than claiming a zero-cost measurement.
    pub fn record(&mut self, result: &TestResult) {
        let failed = match &result.outcome {
            TestOutcome::Passed | TestOutcome::XFailed { .. } => false,
            TestOutcome::Failed { .. } | TestOutcome::XPassed | TestOutcome::Error { .. } => true,
            TestOutcome::Skipped { .. } | TestOutcome::Todo { .. } => return,
        };
        let id = result.test.id();
        let previous = self.file.entries.get(&id).copied();
        let duration =
            if matches!(result.outcome, TestOutcome::Error { .. }) && result.duration.is_zero() {
                previous.and_then(|entry| entry.duration)
            } else {
                Some(result.duration)
            };
        let next = TestRunHistory { failed, duration };
        if previous != Some(next) {
            self.file.entries.insert(id, next);
            self.dirty = true;
        }
    }

    /// Persist changed history atomically. No disk work when nothing
    /// changed since the last successful save or when memory-only.
    ///
    /// # Errors
    ///
    /// Returns any filesystem or encoding error; the in-memory entries are
    /// retained and the next save retries.
    pub fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let bytes = rmp_serde::to_vec_named(&self.file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_cache(path, self.gitignore.as_ref(), &bytes)?;
        self.dirty = false;
        debug!(
            "Scheduling history saved {} entries to {}",
            self.file.entries.len(),
            path.display()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tryke_types::TestItem;

    use super::*;
    use crate::Discoverer;

    fn item(name: &str, case_label: Option<&str>) -> TestItem {
        TestItem {
            name: name.into(),
            module_path: "tests.test_history".into(),
            file_path: Some(PathBuf::from("tests/test_history.py")),
            case_label: case_label.map(Into::into),
            ..TestItem::default()
        }
    }

    fn result(test: &TestItem, outcome: TestOutcome, millis: u64) -> TestResult {
        TestResult {
            test: test.clone(),
            outcome,
            duration: Duration::from_millis(millis),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn failed() -> TestOutcome {
        TestOutcome::Failed {
            message: "boom".into(),
            traceback: None,
            assertions: Vec::new(),
            executed_lines: Vec::new(),
        }
    }

    fn record(failed: bool, millis: Option<u64>) -> TestRunHistory {
        TestRunHistory {
            failed,
            duration: millis.map(Duration::from_millis),
        }
    }

    fn discoverer(root: &Path, cache_dir: Option<&Path>) -> Discoverer {
        Discoverer::from_parts(root, vec![root.to_path_buf()], &[], cache_dir)
    }

    fn canonical_tempdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical tempdir");
        (dir, root)
    }

    #[test]
    fn records_round_trip_through_new_discoverer() {
        let (_dir, root) = canonical_tempdir();
        let slow = item("test_cases", Some("slow"));
        let failing = item("test_cases", Some("failing"));

        let mut history = discoverer(&root, None).load_scheduling_history();
        history.record(&result(&slow, TestOutcome::Passed, 900));
        history.record(&result(&failing, failed(), 10));
        history.save().expect("save");
        assert!(root.join(".tryke/cache/scheduling-v1.bin").exists());

        let mut reloaded = discoverer(&root, None).load_scheduling_history();
        assert_eq!(reloaded.entries().len(), 2);
        assert_eq!(reloaded.entries()[&slow.id()], record(false, Some(900)));
        assert_eq!(reloaded.entries()[&failing.id()], record(true, Some(10)));

        reloaded.record(&result(&failing, TestOutcome::Passed, 5));
        assert_eq!(reloaded.entries()[&failing.id()], record(false, Some(5)));
    }

    #[test]
    fn recording_rules_follow_outcomes() {
        let mut history = SchedulingHistory::default();
        let skipped = item("test_skipped", None);
        let todo = item("test_todo", None);
        let xfailed = item("test_xfailed", None);
        let xpassed = item("test_xpassed", None);
        let error_known = item("test_error_known", None);
        let error_unknown = item("test_error_unknown", None);
        let error_timed = item("test_error_timed", None);
        let zero_pass = item("test_zero_pass", None);
        let zero_fail = item("test_zero_fail", None);
        let untouched = item("test_untouched", None);

        for test in [&skipped, &todo, &error_known, &error_timed, &untouched] {
            history.record(&result(test, TestOutcome::Passed, 300));
        }
        history.record(&result(&untouched, failed(), 40));

        history.record(&result(&skipped, TestOutcome::Skipped { reason: None }, 0));
        history.record(&result(&todo, TestOutcome::Todo { description: None }, 0));
        history.record(&result(&xfailed, TestOutcome::XFailed { reason: None }, 20));
        history.record(&result(&xpassed, TestOutcome::XPassed, 30));
        let error = || TestOutcome::Error {
            message: "spawn failed".into(),
        };
        history.record(&result(&error_known, error(), 0));
        history.record(&result(&error_unknown, error(), 0));
        history.record(&result(&error_timed, error(), 70));
        history.record(&result(&zero_pass, TestOutcome::Passed, 0));
        history.record(&result(&zero_fail, failed(), 0));

        let entries = history.entries();
        assert_eq!(entries[&skipped.id()], record(false, Some(300)));
        assert_eq!(entries[&todo.id()], record(false, Some(300)));
        assert_eq!(entries[&xfailed.id()], record(false, Some(20)));
        assert_eq!(entries[&xpassed.id()], record(true, Some(30)));
        assert_eq!(entries[&error_known.id()], record(true, Some(300)));
        assert_eq!(entries[&error_unknown.id()], record(true, None));
        assert_eq!(entries[&error_timed.id()], record(true, Some(70)));
        assert_eq!(entries[&zero_pass.id()], record(false, Some(0)));
        assert_eq!(entries[&zero_fail.id()], record(true, Some(0)));
        assert_eq!(entries[&untouched.id()], record(true, Some(40)));

        let unseen_skip = item("test_unseen_skip", None);
        history.record(&result(
            &unseen_skip,
            TestOutcome::Skipped { reason: None },
            0,
        ));
        assert!(!history.entries().contains_key(&unseen_skip.id()));
    }

    #[test]
    fn missing_history_loads_empty() {
        let (_dir, root) = canonical_tempdir();
        let history = SchedulingHistory::load(&root, Some(root.join("absent.bin")), None);
        assert!(history.entries().is_empty());
    }

    #[test]
    fn unreadable_history_loads_empty() {
        let (_dir, root) = canonical_tempdir();
        let path = root.join(SCHEDULING_CACHE_FILE_NAME);
        fs::create_dir_all(&path).expect("directory in place of file");
        let history = SchedulingHistory::load(&root, Some(path), None);
        assert!(history.entries().is_empty());
    }

    #[test]
    fn corrupt_history_loads_empty() {
        let (_dir, root) = canonical_tempdir();
        let path = root.join(SCHEDULING_CACHE_FILE_NAME);
        fs::write(&path, b"not msgpack").expect("write");
        let history = SchedulingHistory::load(&root, Some(path), None);
        assert!(history.entries().is_empty());
    }

    #[test]
    fn wrong_version_history_loads_empty() {
        let (_dir, root) = canonical_tempdir();
        let path = root.join(SCHEDULING_CACHE_FILE_NAME);
        let file = HistoryFile {
            version: SCHEDULING_HISTORY_VERSION + 1,
            root: root.clone(),
            entries: HashMap::from([("a".to_owned(), record(true, Some(1)))]),
        };
        fs::write(&path, rmp_serde::to_vec_named(&file).expect("encode")).expect("write");
        let history = SchedulingHistory::load(&root, Some(path), None);
        assert!(history.entries().is_empty());
    }

    #[test]
    fn wrong_root_history_loads_empty() {
        let (_dir, root) = canonical_tempdir();
        let path = root.join(SCHEDULING_CACHE_FILE_NAME);
        let test = item("test_a", None);
        let mut other = SchedulingHistory::load(&root.join("other"), Some(path.clone()), None);
        other.record(&result(&test, TestOutcome::Passed, 1));
        other.save().expect("save");
        assert!(path.exists());

        let history = SchedulingHistory::load(&root, Some(path), None);
        assert!(history.entries().is_empty());
    }

    #[test]
    fn save_writes_only_after_changes() {
        let (_dir, root) = canonical_tempdir();
        let cache_dir = root.join("custom-cache");
        let test = item("test_a", None);
        let mut history = discoverer(&root, Some(&cache_dir)).load_scheduling_history();
        history.record(&result(&test, TestOutcome::Passed, 100));
        history.save().expect("first save");

        // Obstruct the cache directory: any attempted write now fails.
        fs::remove_dir_all(&cache_dir).expect("remove cache dir");
        fs::write(&cache_dir, b"obstruction").expect("obstruct");

        history.save().expect("clean save does no I/O");
        history.record(&result(&test, TestOutcome::Passed, 100));
        history.save().expect("identical record does no I/O");

        history.record(&result(&test, failed(), 200));
        assert!(history.save().is_err());
        assert_eq!(history.entries()[&test.id()], record(true, Some(200)));

        fs::remove_file(&cache_dir).expect("remove obstruction");
        history.save().expect("retry save");
        let reloaded = discoverer(&root, Some(&cache_dir)).load_scheduling_history();
        assert_eq!(reloaded.entries()[&test.id()], record(true, Some(200)));
    }

    #[test]
    fn memory_only_history_records_without_backing_file() {
        let test = item("test_a", None);
        let mut history = SchedulingHistory::default();
        history.record(&result(&test, failed(), 12));
        history.save().expect("memory-only save");
        assert_eq!(
            history.entries(),
            &HashMap::from([(test.id(), record(true, Some(12)))])
        );
    }

    #[test]
    fn history_uses_configured_cache_dir() {
        let (_dir, root) = canonical_tempdir();
        fs::write(
            root.join("pyproject.toml"),
            "[tool.tryke]\ncache_dir = \"configured-cache\"\n",
        )
        .expect("write pyproject");
        let project = tryke_config::Project::discover(&root);
        let test = item("test_a", None);
        let mut history = Discoverer::new(&project).load_scheduling_history();
        history.record(&result(&test, TestOutcome::Passed, 1));
        history.save().expect("save");

        assert!(root.join("configured-cache/scheduling-v1.bin").exists());
        assert!(!root.join(".tryke/cache/scheduling-v1.bin").exists());
        let reloaded = Discoverer::new(&project).load_scheduling_history();
        assert_eq!(reloaded.entries()[&test.id()], record(false, Some(1)));
    }
}
