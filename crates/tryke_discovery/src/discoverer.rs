use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use salsa::Setter;
use tryke_types::TestItem;

use crate::db::{Database, SourceFile, parse_tests};

pub struct Discoverer {
    db: Database,
    inputs: HashMap<PathBuf, SourceFile>,
    root: PathBuf,
}

impl Discoverer {
    #[must_use]
    pub fn new(start: &Path) -> Self {
        let root = crate::find_project_root(start).unwrap_or_else(|| start.to_path_buf());
        Self {
            db: Database::default(),
            inputs: HashMap::new(),
            root,
        }
    }

    pub fn rediscover(&mut self) -> Vec<TestItem> {
        let mut paths = crate::collect_python_files(&self.root);
        paths.sort();
        for path in &paths {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            if let Some(file) = self.inputs.get(path) {
                if file.text(&self.db) != &text {
                    file.set_text(&mut self.db).to(text);
                }
            } else {
                let file = SourceFile::new(&self.db, text, self.root.clone(), path.clone());
                self.inputs.insert(path.clone(), file);
            }
        }
        let path_set: HashSet<&PathBuf> = paths.iter().collect();
        self.inputs.retain(|p, _| path_set.contains(p));
        self.inputs
            .values()
            .flat_map(|f| parse_tests(&self.db, *f))
            .collect()
    }

    pub fn rediscover_changed(&mut self, changed: &[PathBuf]) -> Vec<TestItem> {
        for path in changed {
            if path.extension().is_some_and(|ext| ext == "py") {
                if path.exists() {
                    let text = std::fs::read_to_string(path).unwrap_or_default();
                    if let Some(file) = self.inputs.get(path) {
                        if file.text(&self.db) != &text {
                            file.set_text(&mut self.db).to(text);
                        }
                    } else {
                        let file = SourceFile::new(&self.db, text, self.root.clone(), path.clone());
                        self.inputs.insert(path.clone(), file);
                    }
                } else {
                    self.inputs.remove(path);
                }
            }
        }
        self.inputs
            .values()
            .flat_map(|f| parse_tests(&self.db, *f))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::discover_from;

    fn make_project(files: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        for (rel, content) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create_dir_all");
            }
            fs::write(&path, content).expect("write file");
        }
        dir
    }

    #[test]
    fn discoverer_returns_same_tests_as_discover_from() {
        let source = "@test\ndef test_hello():\n    pass\n";
        let dir = make_project(&[("test_example.py", source)]);
        let mut discoverer = Discoverer::new(dir.path());
        let mut from_discoverer = discoverer.rediscover();
        let mut from_discover = discover_from(dir.path());
        from_discoverer.sort_by(|a, b| a.name.cmp(&b.name));
        from_discover.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(from_discoverer.len(), from_discover.len());
        for (a, b) in from_discoverer.iter().zip(from_discover.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.module_path, b.module_path);
        }
    }

    #[test]
    fn discoverer_picks_up_file_changes() {
        let source_one = "@test\ndef test_one():\n    pass\n";
        let dir = make_project(&[("test_example.py", source_one)]);
        let mut discoverer = Discoverer::new(dir.path());
        let first = discoverer.rediscover();
        assert_eq!(first.len(), 1);
        let source_two = "@test\ndef test_one():\n    pass\n\n@test\ndef test_two():\n    pass\n";
        fs::write(dir.path().join("test_example.py"), source_two).expect("overwrite file");
        let second = discoverer.rediscover();
        assert_eq!(second.len(), 2);
    }

    #[test]
    fn discoverer_removes_deleted_file() {
        let source_a = "@test\ndef test_a():\n    pass\n";
        let source_b = "@test\ndef test_b():\n    pass\n";
        let dir = make_project(&[("test_a.py", source_a), ("test_b.py", source_b)]);
        let mut discoverer = Discoverer::new(dir.path());
        let first = discoverer.rediscover();
        assert_eq!(first.len(), 2);
        fs::remove_file(dir.path().join("test_b.py")).expect("remove file");
        let second = discoverer.rediscover();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].name, "test_a");
    }

    #[test]
    fn discoverer_changed_only_updates_given_paths() {
        let src_a = "@test\ndef test_a():\n    pass\n";
        let src_b = "@test\ndef test_b():\n    pass\n";
        let dir = make_project(&[("test_a.py", src_a), ("test_b.py", src_b)]);
        let mut discoverer = Discoverer::new(dir.path());
        let first = discoverer.rediscover();
        assert_eq!(first.len(), 2);

        // modify both files on disk, but only notify about test_a
        let a_with_extra = "@test\ndef test_a():\n    pass\n\n@test\ndef test_a2():\n    pass\n";
        let b_renamed = "@test\ndef test_b_new():\n    pass\n";
        fs::write(dir.path().join("test_a.py"), a_with_extra).expect("overwrite a");
        fs::write(dir.path().join("test_b.py"), b_renamed).expect("overwrite b");

        let changed = vec![dir.path().join("test_a.py")];
        let second = discoverer.rediscover_changed(&changed);

        // test_a got the new test, but test_b still has old content (not re-read)
        let mut names: Vec<_> = second.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert!(names.contains(&"test_a"), "test_a should be present");
        assert!(
            names.contains(&"test_a2"),
            "test_a2 should be present (new)"
        );
        assert!(names.contains(&"test_b"), "test_b should still be old name");
        assert!(!names.contains(&"test_b_new"), "test_b_new must not appear");
    }

    #[test]
    fn discoverer_changed_removes_deleted_file() {
        let src_a = "@test\ndef test_a():\n    pass\n";
        let src_b = "@test\ndef test_b():\n    pass\n";
        let dir = make_project(&[("test_a.py", src_a), ("test_b.py", src_b)]);
        let mut discoverer = Discoverer::new(dir.path());
        let first = discoverer.rediscover();
        assert_eq!(first.len(), 2);

        let path_b = dir.path().join("test_b.py");
        fs::remove_file(&path_b).expect("remove file");

        let second = discoverer.rediscover_changed(&[path_b]);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].name, "test_a");
    }

    #[test]
    fn discoverer_changed_adds_new_file() {
        let src_a = "@test\ndef test_a():\n    pass\n";
        let dir = make_project(&[("test_a.py", src_a)]);
        let mut discoverer = Discoverer::new(dir.path());
        let first = discoverer.rediscover();
        assert_eq!(first.len(), 1);

        let path_new = dir.path().join("test_new.py");
        fs::write(&path_new, "@test\ndef test_new():\n    pass\n").expect("write new file");

        let second = discoverer.rediscover_changed(&[path_new]);
        assert_eq!(second.len(), 2);
        let mut names: Vec<_> = second.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["test_a", "test_new"]);
    }
}
