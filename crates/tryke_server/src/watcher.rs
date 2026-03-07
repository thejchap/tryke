use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use notify::RecommendedWatcher;
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

#[expect(clippy::missing_errors_doc)]
pub fn spawn_watcher(
    root: &Path,
    tx: mpsc::Sender<Vec<PathBuf>>,
) -> anyhow::Result<Debouncer<RecommendedWatcher>> {
    let mut debouncer = new_debouncer(
        Duration::from_millis(200),
        move |res: DebounceEventResult| {
            if let Ok(events) = res {
                let paths: Vec<PathBuf> = events
                    .iter()
                    .filter(|e| e.path.extension().is_some_and(|ext| ext == "py"))
                    .map(|e| e.path.clone())
                    .collect();
                if !paths.is_empty() {
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
        let _debouncer = spawn_watcher(dir.path(), tx).expect("spawn watcher");

        // give the watcher time to initialize
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
        let _debouncer = spawn_watcher(dir.path(), tx).expect("spawn watcher");

        thread::sleep(Duration::from_millis(100));

        fs::write(&txt_file, "some text").expect("write txt file");

        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "should not fire for non-py files"
        );
    }

    #[test]
    fn watcher_sends_path_of_changed_file() {
        let dir = make_project();
        let py_file = dir.path().join("test_target.py");
        fs::write(&py_file, "@test\ndef foo(): pass").expect("write py file");

        let (tx, rx) = mpsc::channel();
        let _debouncer = spawn_watcher(dir.path(), tx).expect("spawn watcher");

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
