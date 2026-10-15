//! Persistent last-observed outcome/duration per test, used to order
//! work units (previous failures first, then unknown timings, then the
//! longest estimated work). Stored beside the discovery cache.
//!
//! History is advisory: a missing, unreadable, corrupt, wrong-version, or
//! wrong-root file loads empty, and save failures never affect a run.
//!
//! Several owners (CLI runs, watch mode, the server) may share one file.
//! Each save holds an exclusive lock on a sibling `.lock` file, re-reads
//! the latest history, and overlays only the entries this owner changed,
//! so a long-lived owner never overwrites others' newer records.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use log::{debug, trace};
use serde::{Deserialize, Serialize};
use tryke_types::{TestOutcome, TestResult, TestRunHistory};

use super::cache::{GitignoreConfig, prepare_cache, replace_cache};

/// Name of the scheduling history file within the cache directory. The
/// stem is also reused with a `.tmp` extension for the atomic write and a
/// `.lock` extension for the cross-process save lock.
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
    /// Entries changed since the last successful save. Only tracked for
    /// backed stores; a save merges just these onto the latest disk state.
    pending: HashMap<String, TestRunHistory>,
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
            pending: HashMap::new(),
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
            if self.path.is_some() {
                self.pending.insert(id.clone(), next);
            }
            self.file.entries.insert(id, next);
        }
    }

    /// Persist changed entries atomically. No disk work when nothing
    /// changed since the last successful save or when memory-only.
    ///
    /// Under the history lock, the latest file is re-read and only this
    /// owner's changed entries are overlaid, so unrelated updates saved by
    /// other owners survive. For an ID changed by several owners, the last
    /// successful save wins. On success the merged history becomes this
    /// owner's view.
    ///
    /// # Errors
    ///
    /// Returns any lock, filesystem, or encoding error; the in-memory
    /// entries and pending changes are retained and the next save retries.
    pub fn save(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        prepare_cache(path, self.gitignore.as_ref())?;
        let _lock = lock_history(path)?;
        let mut entries = match Self::try_load(&self.file.root, path) {
            Ok(entries) => entries,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) =>
            {
                HashMap::new()
            }
            Err(err) => return Err(err),
        };
        for (id, record) in &self.pending {
            if let Some(entry) = entries.get_mut(id) {
                *entry = *record;
            } else {
                entries.insert(id.clone(), *record);
            }
        }
        let file = HistoryFile {
            version: SCHEDULING_HISTORY_VERSION,
            root: self.file.root.clone(),
            entries,
        };
        let bytes = rmp_serde::to_vec_named(&file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        replace_cache(path, &bytes)?;
        self.file = file;
        self.pending.clear();
        debug!(
            "Scheduling history saved {} entries to {}",
            self.file.entries.len(),
            path.display()
        );
        Ok(())
    }
}

/// Open and exclusively lock the history's sibling `.lock` file, blocking
/// until other cooperating owners release it. The returned handle is the
/// guard; dropping it unlocks. The lock file is never removed or replaced,
/// since a second inode would let two writers hold "the" lock at once.
fn lock_history(path: &Path) -> io::Result<fs::File> {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Child, ChildStdin, Command, Stdio},
        sync::mpsc::{self, RecvTimeoutError},
        thread,
        time::{Duration, Instant},
    };

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

    fn alpha() -> TestItem {
        item("test_cases", Some("alpha"))
    }

    fn beta() -> TestItem {
        item("test_cases", Some("beta"))
    }

    fn untouched() -> TestItem {
        item("test_untouched", None)
    }

    fn baseline() -> HashMap<String, TestRunHistory> {
        HashMap::from([
            (alpha().id(), record(false, Some(900))),
            (beta().id(), record(true, Some(100))),
            (untouched().id(), record(false, Some(60))),
        ])
    }

    /// The result of both `ALPHA` and `BETA` saving over `baseline()`.
    fn alpha_and_beta_merged() -> HashMap<String, TestRunHistory> {
        HashMap::from([
            (alpha().id(), record(true, Some(10))),
            (beta().id(), record(false, Some(200))),
            (untouched().id(), record(false, Some(60))),
        ])
    }

    /// `ALPHA` fails alpha; `BETA` re-records alpha's unchanged baseline
    /// result (a no-op) and passes beta.
    fn apply_role(history: &mut SchedulingHistory, role: &str) -> io::Result<()> {
        match role {
            "ALPHA" => history.record(&result(&alpha(), failed(), 10)),
            "BETA" => {
                history.record(&result(&alpha(), TestOutcome::Passed, 900));
                history.record(&result(&beta(), TestOutcome::Passed, 200));
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown worker role {other:?}"),
                ));
            }
        }
        Ok(())
    }

    fn history_path(cache: &Path) -> PathBuf {
        cache.join(SCHEDULING_CACHE_FILE_NAME)
    }

    fn owner(root: &Path, cache: &Path) -> SchedulingHistory {
        discoverer(root, Some(cache)).load_scheduling_history()
    }

    /// Publish a payload the same way a save replaces the file.
    fn publish(path: &Path, version: u32, root: &Path, entries: HashMap<String, TestRunHistory>) {
        fs::create_dir_all(path.parent().expect("history parent")).expect("cache dir");
        let file = HistoryFile {
            version,
            root: root.to_path_buf(),
            entries,
        };
        let bytes = rmp_serde::to_vec_named(&file).expect("encode");
        replace_cache(path, &bytes).expect("publish history");
    }

    /// Strictly decode the persisted history; unlike `load`, a corrupt
    /// file fails the test instead of reading as empty.
    fn persisted(root: &Path, cache: &Path) -> HashMap<String, TestRunHistory> {
        SchedulingHistory::try_load(root, &history_path(cache)).expect("valid persisted history")
    }

    /// Creates a canonical root with `baseline()` saved in `<root>/cache`.
    fn seeded() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let (dir, root) = canonical_tempdir();
        let cache = root.join("cache");
        publish(
            &history_path(&cache),
            SCHEDULING_HISTORY_VERSION,
            &root,
            baseline(),
        );
        (dir, root, cache)
    }

    #[test]
    fn stale_owners_merge_only_their_changed_entries() {
        let (_dir, root, cache) = seeded();
        let mut a = owner(&root, &cache);
        let mut b = owner(&root, &cache);
        apply_role(&mut a, "ALPHA").expect("alpha role");
        apply_role(&mut b, "BETA").expect("beta role");
        a.save().expect("save A");
        b.save().expect("save B");
        assert_eq!(persisted(&root, &cache), alpha_and_beta_merged());
        assert_eq!(b.entries()[&alpha().id()], record(true, Some(10)));

        a.record(&result(&untouched(), failed(), 30));
        a.save().expect("save A again");
        let mut expected = alpha_and_beta_merged();
        expected.insert(untouched().id(), record(true, Some(30)));
        assert_eq!(persisted(&root, &cache), expected);
    }

    #[test]
    fn same_id_uses_last_successful_save() {
        let (_dir, root) = canonical_tempdir();
        let cache = root.join("cache");
        let x = item("test_x", None);
        publish(
            &history_path(&cache),
            SCHEDULING_HISTORY_VERSION,
            &root,
            HashMap::from([
                (x.id(), record(false, Some(100))),
                (untouched().id(), record(false, Some(60))),
            ]),
        );
        let mut a = owner(&root, &cache);
        let mut b = owner(&root, &cache);
        a.record(&result(&x, failed(), 10));
        a.record(&result(&x, TestOutcome::Passed, 20));
        a.record(&result(&x, failed(), 30));
        b.record(&result(&x, TestOutcome::Passed, 5));

        a.save().expect("save A");
        let after_a = persisted(&root, &cache);
        assert_eq!(after_a[&x.id()], record(true, Some(30)));
        b.save().expect("save B");
        let after_b = persisted(&root, &cache);
        assert_eq!(after_b[&x.id()], record(false, Some(5)));
        assert_eq!(after_b[&untouched().id()], record(false, Some(60)));
    }

    #[test]
    fn successful_save_does_not_replay_old_pending_records() {
        let (_dir, root) = canonical_tempdir();
        let cache = root.join("cache");
        let x = item("test_x", None);
        let y = item("test_y", None);
        publish(
            &history_path(&cache),
            SCHEDULING_HISTORY_VERSION,
            &root,
            HashMap::from([
                (x.id(), record(false, Some(100))),
                (y.id(), record(false, Some(60))),
            ]),
        );
        let mut a = owner(&root, &cache);
        let mut b = owner(&root, &cache);
        a.record(&result(&x, failed(), 10));
        a.save().expect("save A");
        b.record(&result(&x, TestOutcome::Passed, 20));
        b.save().expect("save B");
        a.record(&result(&y, failed(), 30));
        a.save().expect("save A again");
        assert_eq!(
            persisted(&root, &cache),
            HashMap::from([
                (x.id(), record(false, Some(20))),
                (y.id(), record(true, Some(30))),
            ])
        );
    }

    #[test]
    fn save_replaces_invalid_history_but_preserves_pending_on_io_errors() {
        type Invalidate = fn(&Path, &Path);
        let only_alpha = || HashMap::from([(alpha().id(), record(true, Some(10)))]);
        let invalidations: [(&str, Invalidate); 4] = [
            ("missing", |path, _| fs::remove_file(path).expect("remove")),
            ("invalid msgpack", |path, _| {
                fs::write(path, b"not msgpack").expect("write");
            }),
            ("unsupported version", |path, root| {
                publish(path, SCHEDULING_HISTORY_VERSION + 1, root, baseline());
            }),
            ("different root", |path, root| {
                publish(
                    path,
                    SCHEDULING_HISTORY_VERSION,
                    &root.join("other"),
                    baseline(),
                );
            }),
        ];
        for (name, invalidate) in invalidations {
            let (_dir, root, cache) = seeded();
            let mut history = owner(&root, &cache);
            apply_role(&mut history, "ALPHA").expect("alpha role");
            invalidate(&history_path(&cache), &root);
            history
                .save()
                .unwrap_or_else(|err| panic!("{name}: save failed: {err}"));
            assert_eq!(persisted(&root, &cache), only_alpha(), "{name}");
        }

        let (_dir, root, cache) = seeded();
        let path = history_path(&cache);
        let mut history = owner(&root, &cache);
        apply_role(&mut history, "ALPHA").expect("alpha role");
        let local = history.entries().clone();
        fs::remove_file(&path).expect("remove history");
        fs::create_dir(&path).expect("obstruct history");
        assert!(history.save().is_err());
        assert_eq!(history.entries(), &local);

        fs::remove_dir(&path).expect("remove obstruction");
        history.save().expect("retry save");
        assert_eq!(persisted(&root, &cache), only_alpha());
    }

    #[test]
    fn failed_merge_save_retries_without_losing_updates() {
        let (_dir, root, cache) = seeded();
        let path = history_path(&cache);
        let mut a = owner(&root, &cache);
        apply_role(&mut a, "ALPHA").expect("alpha role");
        let tmp = path.with_extension("tmp");
        fs::create_dir(&tmp).expect("obstruct temp file");
        assert!(a.save().is_err());
        assert_eq!(persisted(&root, &cache), baseline());
        assert_eq!(a.entries()[&alpha().id()], record(true, Some(10)));

        fs::remove_dir(&tmp).expect("remove obstruction");
        let mut b = Worker::spawn(&root, &cache, "BETA", false);
        b.expect(READY);
        b.send_save();
        b.finish();

        a.save().expect("retry save");
        assert_eq!(persisted(&root, &cache), alpha_and_beta_merged());
    }

    #[test]
    fn concurrent_process_saves_preserve_all_updates() {
        let (_dir, root, cache) = seeded();
        let path = history_path(&cache);
        let lock = lock_history(&path).expect("parent lock");
        let mut workers = [
            Worker::spawn(&root, &cache, "ALPHA", true),
            Worker::spawn(&root, &cache, "BETA", true),
        ];
        // Both workers hold stale snapshots before either may save.
        for worker in &workers {
            worker.expect(READY);
        }
        for worker in &mut workers {
            worker.send_save();
        }
        for worker in &workers {
            worker.expect(CONTENDED);
        }
        drop(lock);
        for worker in &mut workers {
            worker.finish();
        }
        assert_eq!(persisted(&root, &cache), alpha_and_beta_merged());
        assert!(!path.with_extension("tmp").exists());
    }

    #[test]
    fn save_merges_history_read_after_lock_acquisition() {
        let (_dir, root, cache) = seeded();
        let path = history_path(&cache);
        let lock = lock_history(&path).expect("parent lock");
        let mut worker = Worker::spawn(&root, &cache, "ALPHA", true);
        worker.expect(READY);
        worker.send_save();
        worker.expect(CONTENDED);

        let mut newer = baseline();
        newer.insert(beta().id(), record(false, Some(250)));
        publish(&path, SCHEDULING_HISTORY_VERSION, &root, newer);
        drop(lock);
        worker.finish();

        assert_eq!(
            persisted(&root, &cache),
            HashMap::from([
                (alpha().id(), record(true, Some(10))),
                (beta().id(), record(false, Some(250))),
                (untouched().id(), record(false, Some(60))),
            ])
        );
    }

    const WORKER_TEST: &str = "filesystem::history::tests::scheduling_history_process_worker";
    const ROOT_ENV: &str = "TRYKE_SCHEDULING_HISTORY_TEST_ROOT";
    const CACHE_ENV: &str = "TRYKE_SCHEDULING_HISTORY_TEST_CACHE";
    const ROLE_ENV: &str = "TRYKE_SCHEDULING_HISTORY_TEST_ROLE";
    const CONTENDED_ENV: &str = "TRYKE_SCHEDULING_HISTORY_TEST_CONTENDED";
    const READY: &str = "TRYKE_HISTORY:READY";
    const CONTENDED: &str = "TRYKE_HISTORY:CONTENDED";
    const SAVED: &str = "TRYKE_HISTORY:SAVED";
    /// Deadlock bound for child coordination, not a speed assertion.
    const PROCESS_DEADLINE: Duration = Duration::from_secs(30);
    const EXIT_POLL: Duration = Duration::from_millis(20);

    fn emit(marker: &str) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{marker}")?;
        stdout.flush()
    }

    fn worker_env(key: &str) -> io::Result<String> {
        std::env::var(key).map_err(|err| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "{key}: {err}; this test only runs as a child of the history process tests"
                ),
            )
        })
    }

    /// Child-process entry point: loads and records a role's changes,
    /// reports `READY`, then saves when the parent sends `SAVE`.
    #[test]
    #[ignore = "child process driven by the scheduling history process tests"]
    fn scheduling_history_process_worker() -> io::Result<()> {
        let root = PathBuf::from(worker_env(ROOT_ENV)?);
        let cache = PathBuf::from(worker_env(CACHE_ENV)?);
        let role = worker_env(ROLE_ENV)?;
        let contended = std::env::var(CONTENDED_ENV).is_ok_and(|value| value == "1");

        let mut history = owner(&root, &cache);
        apply_role(&mut history, &role)?;
        emit(READY)?;

        let mut command = String::new();
        io::stdin().read_line(&mut command)?;
        if command.trim_end() != "SAVE" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpected command {command:?}"),
            ));
        }
        if contended {
            let probe = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(history_path(&cache).with_extension("lock"))?;
            match probe.try_lock() {
                Err(fs::TryLockError::WouldBlock) => {}
                Err(fs::TryLockError::Error(err)) => return Err(err),
                Ok(()) => {
                    return Err(io::Error::other("history lock was not held by the parent"));
                }
            }
            drop(probe);
            emit(CONTENDED)?;
        }
        history.save()?;
        emit(SAVED)
    }

    /// A libtest child running `scheduling_history_process_worker`, killed
    /// and reaped on drop so a failed assertion never leaks a process.
    struct Worker {
        child: Child,
        stdin: Option<ChildStdin>,
        lines: mpsc::Receiver<String>,
    }

    impl Worker {
        fn spawn(root: &Path, cache: &Path, role: &str, contended: bool) -> Self {
            let mut command = Command::new(std::env::current_exe().expect("test binary"));
            command
                .args([
                    "--exact",
                    WORKER_TEST,
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(ROOT_ENV, root)
                .env(CACHE_ENV, cache)
                .env(ROLE_ENV, role)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
            if contended {
                command.env(CONTENDED_ENV, "1");
            }
            let mut child = command.spawn().expect("spawn history worker");
            let stdin = child.stdin.take();
            let stdout = child.stdout.take();
            let (tx, lines) = mpsc::channel();
            let worker = Self {
                child,
                stdin,
                lines,
            };
            let stdout = stdout.expect("piped worker stdout");
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
            worker
        }

        /// Wait for `marker`, skipping libtest's own output. It may share
        /// a line with libtest's test-name prefix.
        fn expect(&self, marker: &str) {
            let deadline = Instant::now() + PROCESS_DEADLINE;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "timed out waiting for {marker}");
                match self.lines.recv_timeout(remaining) {
                    Ok(line) if line.contains(marker) => return,
                    Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        panic!("worker closed stdout before {marker}");
                    }
                }
            }
        }

        fn send_save(&mut self) {
            let stdin = self.stdin.as_mut().expect("worker stdin");
            writeln!(stdin, "SAVE").expect("send SAVE");
            stdin.flush().expect("flush SAVE");
        }

        fn finish(&mut self) {
            self.expect(SAVED);
            self.stdin = None;
            let deadline = Instant::now() + PROCESS_DEADLINE;
            loop {
                if let Some(status) = self.child.try_wait().expect("poll worker") {
                    assert!(status.success(), "worker exited with {status}");
                    return;
                }
                assert!(Instant::now() < deadline, "worker did not exit");
                thread::sleep(EXIT_POLL);
            }
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
