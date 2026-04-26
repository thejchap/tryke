use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use log::debug;
use notify::RecommendedWatcher;
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

// Tests use a shorter delay so the suite doesn't pay 200ms per watcher case;
// the real watcher needs 200ms to coalesce bursty editor saves.
#[cfg(not(test))]
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);
#[cfg(test)]
const DEBOUNCE_DELAY: Duration = Duration::from_millis(50);

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
    let mut debouncer = new_debouncer(DEBOUNCE_DELAY, move |res: DebounceEventResult| {
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
    })?;
    debouncer
        .watcher()
        .watch(root, notify::RecursiveMode::Recursive)?;
    Ok(debouncer)
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
            rx.recv_timeout(Duration::from_millis(400)).is_err(),
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
            rx.recv_timeout(Duration::from_millis(400)).is_err(),
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
}
