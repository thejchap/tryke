use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, SystemTime},
};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use log::debug;
use notify::RecommendedWatcher;
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

fn build_gitignore(root: &Path, excludes: &[String]) -> Gitignore {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut builder = GitignoreBuilder::new(&canonical);
    let _ = builder.add(canonical.join(".gitignore"));
    let _ = builder.add(canonical.join(".ignore"));
    for exclude in excludes {
        let _ = builder.add_line(None, exclude);
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

#[expect(clippy::missing_errors_doc)]
pub fn spawn_watcher(
    root: &Path,
    excludes: &[String],
    tx: mpsc::Sender<Vec<PathBuf>>,
) -> anyhow::Result<Debouncer<RecommendedWatcher>> {
    let gitignore = build_gitignore(root, excludes);
    // 100ms is enough to coalesce the burst of inotify events the
    // kernel emits for a single write syscall (typically sub-ms apart).
    // We rely on `ChangeFilter` further downstream — not on a wide
    // debounce window — to suppress duplicate restarts from editor
    // tail activity that arrives after this window.
    let mut debouncer = new_debouncer(
        Duration::from_millis(100),
        move |res: DebounceEventResult| {
            if let Ok(events) = res {
                let paths: Vec<PathBuf> = events
                    .iter()
                    .filter(|e| e.path.extension().is_some_and(|ext| ext == "py"))
                    .filter(|e| {
                        !gitignore
                            .matched_path_or_any_parents(&e.path, false)
                            .is_ignore()
                    })
                    .map(|e| e.path.clone())
                    .collect();
                if !paths.is_empty() {
                    debug!("file changes detected: {paths:?}");
                    let _ = tx.send(paths);
                }
            }
        },
    )?;
    debouncer
        .watcher()
        .watch(root, notify::RecursiveMode::Recursive)?;
    Ok(debouncer)
}

/// Cheap content signature used to decide whether a watcher event
/// represents a real change. We deliberately match what the discovery
/// disk cache uses (`tryke_discovery::cache::FileKey`) so that any
/// file the discovery layer would treat as "unchanged" is also
/// treated as "unchanged" here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileSig {
    mtime_nanos: i128,
    size: u64,
}

impl FileSig {
    fn from_path(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        let mtime = m.modified().ok()?;
        let mtime_nanos = match mtime.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
            Err(e) => -i128::try_from(e.duration().as_nanos()).unwrap_or(i128::MAX),
        };
        Some(Self {
            mtime_nanos,
            size: m.len(),
        })
    }
}

/// Drops watcher events that don't reflect a real content change.
///
/// `notify-debouncer-mini` coalesces inotify bursts within its 200ms
/// quiet window, but a single editor save can still produce two batches
/// when the editor's tail activity (metadata fsync, swap-file cleanup,
/// format-on-save with identical output, LSP write) lands outside that
/// window. Without dedup, each batch triggers its own restart cycle —
/// the user perceives one save as two restarts.
///
/// `ChangeFilter` answers the deterministic question: "did the file's
/// `(mtime, size)` actually move since the last batch we accepted?"
/// Same primitive the discovery cache uses to skip re-parsing unchanged
/// files. Tail events that don't move the signature are silently
/// dropped; genuine second saves (different content → different mtime)
/// still flow through.
#[derive(Debug, Default)]
pub struct ChangeFilter {
    last_seen: HashMap<PathBuf, FileSig>,
}

impl ChangeFilter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return only the paths whose current sig differs from the
    /// previously-stamped sig (or that have no stamp yet). Updates
    /// stamps for every returned path.
    ///
    /// `None` from `FileSig::from_path` (file deleted, unreadable)
    /// is treated as a state — a transition between `Some` and `None`
    /// counts as a change too, so a deletion is reported once and
    /// then quiesces if no further action is taken on the path.
    pub fn filter(&mut self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut out = Vec::with_capacity(paths.len());
        for path in paths {
            let current = FileSig::from_path(path);
            let prior = self.last_seen.get(path).copied();
            if current != prior {
                match current {
                    Some(sig) => {
                        self.last_seen.insert(path.clone(), sig);
                    }
                    None => {
                        self.last_seen.remove(path);
                    }
                }
                out.push(path.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::mpsc, thread, time::Duration};

    use tempfile::TempDir;

    use super::*;

    fn make_project() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        dir
    }

    #[test]
    fn watcher_fires_on_py_file_change() {
        let dir = make_project();
        let py_file = dir.path().join("test.py");
        fs::write(&py_file, "@test\ndef foo(): pass").expect("write py file");

        let (tx, rx) = mpsc::channel();
        let _debouncer = spawn_watcher(dir.path(), &[], tx).expect("spawn watcher");

        // Give the watcher time to initialize
        thread::sleep(Duration::from_millis(100));

        fs::write(&py_file, "@test\ndef bar(): pass").expect("update py file");

        assert!(
            rx.recv_timeout(Duration::from_secs(3)).is_ok(),
            "expected file change notification"
        );
    }

    #[test]
    fn watcher_ignores_non_py_files() {
        let dir = make_project();
        let txt_file = dir.path().join("notes.txt");

        let (tx, rx) = mpsc::channel();
        let _debouncer = spawn_watcher(dir.path(), &[], tx).expect("spawn watcher");

        thread::sleep(Duration::from_millis(100));

        fs::write(&txt_file, "some text").expect("write txt file");

        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "should not fire for non-py files"
        );
    }

    #[test]
    fn watcher_ignores_gitignored_py_files() {
        let dir = make_project();
        let venv_dir = dir.path().join(".venv");
        fs::create_dir_all(&venv_dir).expect("create .venv");
        fs::write(dir.path().join(".gitignore"), ".venv/\n").expect("write .gitignore");
        let ignored_file = venv_dir.join("lib.py");
        fs::write(&ignored_file, "x = 1").expect("write ignored py file");

        let (tx, rx) = mpsc::channel();
        let _debouncer = spawn_watcher(dir.path(), &[], tx).expect("spawn watcher");

        thread::sleep(Duration::from_millis(100));

        fs::write(&ignored_file, "x = 2").expect("update ignored py file");

        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "should not fire for gitignored py files"
        );
    }

    #[test]
    fn watcher_sends_path_of_changed_file() {
        let dir = make_project();
        let py_file = dir.path().join("test_target.py");
        fs::write(&py_file, "@test\ndef foo(): pass").expect("write py file");

        let (tx, rx) = mpsc::channel();
        let _debouncer = spawn_watcher(dir.path(), &[], tx).expect("spawn watcher");

        thread::sleep(Duration::from_millis(100));

        fs::write(&py_file, "@test\ndef bar(): pass").expect("update py file");

        let paths = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("expected notification");
        let canonical = py_file.canonicalize().unwrap_or(py_file.clone());
        assert!(
            paths.iter().any(|p| p == &py_file || p == &canonical),
            "expected changed path in notification, got {paths:?}"
        );
    }

    /// Bump mtime on `path` until `FileSig::from_path` actually
    /// observes a different sig than `prior`. Filesystem mtime
    /// resolution varies by platform (1ns on Linux ext4, 1µs on
    /// some macOS configs), so a `fs::write` immediately followed by
    /// another `fs::write` can produce identical mtimes.
    fn bump_until_sig_changes(path: &Path, prior: FileSig) {
        for i in 0u32..100 {
            fs::write(path, format!("@test\ndef bumped_{i}(): pass\n# {i:08}\n")).expect("write");
            if let Some(now) = FileSig::from_path(path)
                && now != prior
            {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "could not bump mtime/size on {} after 100 attempts",
            path.display()
        );
    }

    #[test]
    fn change_filter_first_observation_is_change() {
        let dir = make_project();
        let py = dir.path().join("a.py");
        fs::write(&py, "x = 1").expect("write");

        let mut f = ChangeFilter::new();
        let first = f.filter(std::slice::from_ref(&py));
        assert_eq!(first, vec![py.clone()], "first sighting must report");
        let second = f.filter(std::slice::from_ref(&py));
        assert!(
            second.is_empty(),
            "no-op event with unchanged sig must be dropped, got {second:?}"
        );
    }

    #[test]
    fn change_filter_detects_mtime_change() {
        let dir = make_project();
        let py = dir.path().join("a.py");
        fs::write(&py, "x = 1").expect("write");

        let mut f = ChangeFilter::new();
        let _ = f.filter(std::slice::from_ref(&py));
        let prior = FileSig::from_path(&py).expect("sig");

        bump_until_sig_changes(&py, prior);

        let after = f.filter(std::slice::from_ref(&py));
        assert_eq!(
            after,
            vec![py],
            "real content change must be reported as changed"
        );
    }

    #[test]
    fn change_filter_ignores_no_op_event() {
        let dir = make_project();
        let py = dir.path().join("a.py");
        fs::write(&py, "x = 1").expect("write");

        let mut f = ChangeFilter::new();
        let _ = f.filter(std::slice::from_ref(&py));
        // Re-fire with no underlying FS change at all.
        let again = f.filter(std::slice::from_ref(&py));
        let again2 = f.filter(std::slice::from_ref(&py));
        assert!(
            again.is_empty(),
            "no-op event #1 should drop, got {again:?}"
        );
        assert!(
            again2.is_empty(),
            "no-op event #2 should drop, got {again2:?}"
        );
    }

    #[test]
    fn change_filter_detects_deletion() {
        let dir = make_project();
        let py = dir.path().join("a.py");
        fs::write(&py, "x = 1").expect("write");

        let mut f = ChangeFilter::new();
        let _ = f.filter(std::slice::from_ref(&py));

        fs::remove_file(&py).expect("rm");
        let after_delete = f.filter(std::slice::from_ref(&py));
        assert_eq!(
            after_delete,
            vec![py.clone()],
            "deletion (Some -> None) must be reported once"
        );
        let after_quiet = f.filter(std::slice::from_ref(&py));
        assert!(
            after_quiet.is_empty(),
            "second event on still-deleted path is a no-op, got {after_quiet:?}"
        );
    }

    #[test]
    fn change_filter_only_returns_changed_paths_in_mixed_batch() {
        let dir = make_project();
        let a = dir.path().join("a.py");
        let b = dir.path().join("b.py");
        fs::write(&a, "x = 1").expect("write a");
        fs::write(&b, "y = 1").expect("write b");

        let mut f = ChangeFilter::new();
        let _ = f.filter(&[a.clone(), b.clone()]);

        let prior_a = FileSig::from_path(&a).expect("sig a");
        bump_until_sig_changes(&a, prior_a);

        let mixed = f.filter(&[a.clone(), b]);
        assert_eq!(
            mixed,
            vec![a],
            "only the path that actually moved should be reported"
        );
    }
}
