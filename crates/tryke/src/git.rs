use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

fn git_paths(root: &Path, args: &[&str]) -> Option<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let paths: Vec<PathBuf> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| root.join(l))
        .collect();
    Some(paths)
}

/// Collect changed files from git relative to `root`.
/// Includes tracked changes since HEAD and untracked files.
/// Returns `None` if git is unavailable or a command fails.
pub fn git_changed_files(root: &Path) -> Option<Vec<PathBuf>> {
    let tracked = git_paths(root, &["diff", "--name-only", "HEAD"])?;
    let untracked = git_paths(root, &["ls-files", "--others", "--exclude-standard"])?;
    let mut paths: Vec<PathBuf> = tracked
        .into_iter()
        .chain(untracked)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    paths.sort();
    Some(paths)
}

/// Collect files changed on the current branch relative to `base`.
/// Uses three-dot merge-base diff so only the branch's own changes appear.
/// Also includes untracked files (not captured by the diff).
/// Returns `None` if git is unavailable or a command fails.
pub fn git_branch_changed_files(root: &Path, base: &str) -> Option<Vec<PathBuf>> {
    let diff_spec = format!("{base}...HEAD");
    let branch_diff = git_paths(root, &["diff", "--name-only", &diff_spec])?;
    let untracked = git_paths(root, &["ls-files", "--others", "--exclude-standard"])?;
    let mut paths: Vec<PathBuf> = branch_diff
        .into_iter()
        .chain(untracked)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    paths.sort();
    Some(paths)
}

/// Resolve changed files using either branch mode or HEAD mode.
pub fn resolve_changed_files(root: &Path, base_branch: Option<&str>) -> Option<Vec<PathBuf>> {
    match base_branch {
        Some(base) => git_branch_changed_files(root, base),
        None => git_changed_files(root),
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::path::Path;

    pub(crate) fn init_git_repo(dir: &Path) {
        fn run(dir: &Path, args: &[&str]) {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .expect("run git");
            assert!(status.success(), "git {:?} failed", args);
        }

        run(dir, &["init"]);
        run(dir, &["config", "user.email", "tryke@example.com"]);
        run(dir, &["config", "user.name", "Tryke Tests"]);
        run(dir, &["config", "commit.gpgsign", "false"]);
    }

    pub(crate) fn git_run(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    /// Seed a git repo with an initial commit containing `pyproject.toml`
    /// and an optional set of extra files, so `git diff --name-only HEAD`
    /// has a baseline to compare against.
    pub(crate) fn seed_git_repo(dir: &Path, files: &[(&str, &str)]) {
        std::fs::write(dir.join("pyproject.toml"), "").expect("write pyproject.toml");
        for &(name, content) in files {
            if let Some(parent) = Path::new(name).parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(dir.join(parent)).expect("mkdir");
            }
            std::fs::write(dir.join(name), content).expect("write file");
        }
        init_git_repo(dir);
        git_run(dir, &["add", "."]);
        git_run(dir, &["commit", "-m", "initial"]);
    }

    /// Seed a git repo with an explicit "main" branch name for branch-mode tests.
    pub(crate) fn seed_git_repo_with_main(dir: &Path, files: &[(&str, &str)]) {
        std::fs::write(dir.join("pyproject.toml"), "").expect("write pyproject.toml");
        for &(name, content) in files {
            if let Some(parent) = Path::new(name).parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(dir.join(parent)).expect("mkdir");
            }
            std::fs::write(dir.join(name), content).expect("write file");
        }
        init_git_repo(dir);
        git_run(dir, &["checkout", "-b", "main"]);
        git_run(dir, &["add", "."]);
        git_run(dir, &["commit", "-m", "initial"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_helpers::*;

    #[test]
    fn git_changed_files_includes_untracked() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(dir.path(), &[("tracked.py", "def helper(): pass\n")]);

        std::fs::write(
            dir.path().join("test_new.py"),
            "@test\ndef test_new(): pass\n",
        )
        .expect("write untracked file");

        let changed = git_changed_files(dir.path()).expect("git changed files");
        assert!(changed.contains(&dir.path().join("test_new.py")));
    }

    #[test]
    fn git_changed_files_includes_tracked_modifications() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(dir.path(), &[("lib.py", "x = 1\n")]);

        std::fs::write(dir.path().join("lib.py"), "x = 2\n").expect("modify tracked file");

        let changed = git_changed_files(dir.path()).expect("git changed files");
        assert!(changed.contains(&dir.path().join("lib.py")));
    }

    #[test]
    fn git_changed_files_returns_empty_when_clean() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(dir.path(), &[("lib.py", "x = 1\n")]);

        let changed = git_changed_files(dir.path()).expect("git changed files");
        assert!(changed.is_empty());
    }

    #[test]
    fn git_changed_files_includes_staged_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(dir.path(), &[]);

        std::fs::write(dir.path().join("new.py"), "y = 1\n").expect("write new file");
        git_run(dir.path(), &["add", "new.py"]);

        let changed = git_changed_files(dir.path()).expect("git changed files");
        assert!(changed.contains(&dir.path().join("new.py")));
    }

    #[test]
    fn git_changed_files_deduplicates_tracked_and_untracked() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(dir.path(), &[]);

        // Stage a new file, then modify it again so it appears in both
        // `git diff --name-only HEAD` (staged) and `git ls-files --others`
        // would not list it since it's tracked. But the staged version differs
        // from HEAD, and the working copy differs from the index, so it
        // appears in `git diff --name-only HEAD` (which covers both).
        // This verifies our BTreeSet dedup doesn't produce duplicates.
        std::fs::write(dir.path().join("dup.py"), "v1\n").expect("write");
        git_run(dir.path(), &["add", "dup.py"]);
        std::fs::write(dir.path().join("dup.py"), "v2\n").expect("modify");

        let changed = git_changed_files(dir.path()).expect("git changed files");
        let count = changed
            .iter()
            .filter(|p| *p == &dir.path().join("dup.py"))
            .count();
        assert_eq!(count, 1, "file should appear exactly once");
    }

    #[test]
    fn git_changed_files_handles_paths_with_spaces() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(dir.path(), &[]);

        let subdir = dir.path().join("my tests");
        std::fs::create_dir_all(&subdir).expect("mkdir");
        std::fs::write(subdir.join("test_space.py"), "pass\n").expect("write");

        let changed = git_changed_files(dir.path()).expect("git changed files");
        assert!(
            changed.contains(&subdir.join("test_space.py")),
            "should detect files in directories with spaces: {changed:?}"
        );
    }

    #[test]
    fn git_changed_files_includes_deleted_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(dir.path(), &[("to_delete.py", "pass\n")]);

        std::fs::remove_file(dir.path().join("to_delete.py")).expect("delete file");

        let changed = git_changed_files(dir.path()).expect("git changed files");
        assert!(
            changed.contains(&dir.path().join("to_delete.py")),
            "deleted files should appear in changed list: {changed:?}"
        );
    }

    #[test]
    fn git_changed_files_returns_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(dir.path(), &[]);

        std::fs::write(dir.path().join("z.py"), "pass\n").expect("write");
        std::fs::write(dir.path().join("a.py"), "pass\n").expect("write");
        std::fs::write(dir.path().join("m.py"), "pass\n").expect("write");

        let changed = git_changed_files(dir.path()).expect("git changed files");
        let is_sorted = changed.windows(2).all(|w| w[0] <= w[1]);
        assert!(is_sorted, "results should be sorted: {changed:?}");
    }

    // --- Branch mode tests ---

    #[test]
    fn git_branch_changed_files_returns_branch_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo_with_main(dir.path(), &[("base.py", "x = 1\n")]);

        git_run(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("feature.py"), "y = 2\n").expect("write");
        git_run(dir.path(), &["add", "feature.py"]);
        git_run(dir.path(), &["commit", "-m", "feature commit"]);

        let changed =
            git_branch_changed_files(dir.path(), "main").expect("git branch changed files");
        assert!(
            changed.contains(&dir.path().join("feature.py")),
            "branch file should appear: {changed:?}"
        );
        assert!(
            !changed.contains(&dir.path().join("base.py")),
            "base file should not appear: {changed:?}"
        );
    }

    #[test]
    fn git_branch_changed_files_includes_untracked() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo_with_main(dir.path(), &[("base.py", "x = 1\n")]);

        git_run(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("feature.py"), "y = 2\n").expect("write");
        git_run(dir.path(), &["add", "feature.py"]);
        git_run(dir.path(), &["commit", "-m", "feature commit"]);

        std::fs::write(dir.path().join("untracked.py"), "z = 3\n").expect("write");

        let changed =
            git_branch_changed_files(dir.path(), "main").expect("git branch changed files");
        assert!(
            changed.contains(&dir.path().join("untracked.py")),
            "untracked file should appear: {changed:?}"
        );
    }

    #[test]
    fn git_branch_changed_files_nonexistent_branch_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo(dir.path(), &[("base.py", "x = 1\n")]);

        let result = git_branch_changed_files(dir.path(), "nonexistent-branch-xyz");
        assert!(result.is_none(), "nonexistent branch should return None");
    }

    #[test]
    fn git_branch_changed_files_excludes_main_only_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_git_repo_with_main(dir.path(), &[("base.py", "x = 1\n")]);

        git_run(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("feature.py"), "y = 2\n").expect("write");
        git_run(dir.path(), &["add", "feature.py"]);
        git_run(dir.path(), &["commit", "-m", "feature commit"]);

        // Switch back to main and add a change there
        git_run(dir.path(), &["checkout", "main"]);
        std::fs::write(dir.path().join("main_only.py"), "z = 3\n").expect("write");
        git_run(dir.path(), &["add", "main_only.py"]);
        git_run(dir.path(), &["commit", "-m", "main-only commit"]);

        // Switch back to feature branch
        git_run(dir.path(), &["checkout", "feature"]);

        let changed =
            git_branch_changed_files(dir.path(), "main").expect("git branch changed files");
        assert!(
            !changed.contains(&dir.path().join("main_only.py")),
            "main-only changes should not appear: {changed:?}"
        );
        assert!(
            changed.contains(&dir.path().join("feature.py")),
            "feature changes should appear: {changed:?}"
        );
    }
}
