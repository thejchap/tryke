use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use log::debug;
use salsa::Setter;
use tryke_types::TestItem;

use crate::{
    db::{Database, SourceFile, parse_tests},
    import_graph::{GraphEntry, ImportGraph},
};

pub struct Discoverer {
    db: Database,
    inputs: HashMap<PathBuf, SourceFile>,
    root: PathBuf,
    import_graph: ImportGraph,
}

impl Discoverer {
    #[must_use]
    pub fn new(start: &Path) -> Self {
        let root = crate::find_project_root(start).unwrap_or_else(|| start.to_path_buf());
        let root = root.canonicalize().unwrap_or(root);
        Self {
            db: Database::default(),
            inputs: HashMap::new(),
            root,
            import_graph: ImportGraph::default(),
        }
    }

    pub fn rediscover(&mut self) -> Vec<TestItem> {
        let mut paths = crate::collect_python_files(&self.root);
        paths.sort();
        debug!(
            "rediscover: found {} python files in {}",
            paths.len(),
            self.root.display()
        );
        for path in &paths {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let imports = crate::extract_local_imports(&self.root, path, &text);
            if let Some(file) = self.inputs.get(path) {
                if file.text(&self.db) != &text {
                    debug!("rediscover: re-parsing changed file {}", path.display());
                    file.set_text(&mut self.db).to(text);
                }
            } else {
                debug!("rediscover: parsing new file {}", path.display());
                let file = SourceFile::new(&self.db, text, self.root.clone(), path.clone());
                self.inputs.insert(path.clone(), file);
            }
            self.import_graph.update(path.clone(), imports);
        }
        let path_set: HashSet<&PathBuf> = paths.iter().collect();
        let removed: Vec<PathBuf> = self
            .inputs
            .keys()
            .filter(|p| !path_set.contains(p))
            .cloned()
            .collect();
        for path in removed {
            self.import_graph.remove(&path);
            self.inputs.remove(&path);
        }
        let tests: Vec<TestItem> = self
            .inputs
            .values()
            .flat_map(|f| parse_tests(&self.db, *f))
            .collect();
        debug!("rediscover: discovered {} tests total", tests.len());
        tests
    }

    pub fn tests(&self) -> Vec<TestItem> {
        self.inputs
            .values()
            .flat_map(|f| parse_tests(&self.db, *f))
            .collect()
    }

    pub fn rediscover_changed(&mut self, changed: &[PathBuf]) -> Vec<TestItem> {
        let changed = Self::canonicalize_paths(changed);
        debug!(
            "rediscover_changed: processing {} changed paths",
            changed.len()
        );
        for path in &changed {
            if path.extension().is_some_and(|ext| ext == "py") {
                if path.exists() {
                    let text = std::fs::read_to_string(path).unwrap_or_default();
                    let imports = crate::extract_local_imports(&self.root, path, &text);
                    if let Some(file) = self.inputs.get(path) {
                        if file.text(&self.db) != &text {
                            debug!(
                                "rediscover_changed: re-parsing changed file {}",
                                path.display()
                            );
                            file.set_text(&mut self.db).to(text);
                        }
                    } else {
                        debug!("rediscover_changed: parsing new file {}", path.display());
                        let file = SourceFile::new(&self.db, text, self.root.clone(), path.clone());
                        self.inputs.insert(path.clone(), file);
                    }
                    self.import_graph.update(path.clone(), imports);
                } else {
                    debug!(
                        "rediscover_changed: removing deleted file {}",
                        path.display()
                    );
                    self.import_graph.remove(path);
                    self.inputs.remove(path);
                }
            }
        }
        let tests: Vec<TestItem> = self
            .inputs
            .values()
            .flat_map(|f| parse_tests(&self.db, *f))
            .collect();
        debug!("rediscover_changed: {} tests after update", tests.len());
        tests
    }

    fn canonicalize_path(p: &Path) -> PathBuf {
        if let Ok(c) = p.canonicalize() {
            return c;
        }
        // file may be deleted; canonicalize parent + filename
        if let (Some(parent), Some(name)) = (p.parent(), p.file_name())
            && let Ok(cp) = parent.canonicalize()
        {
            return cp.join(name);
        }
        p.to_path_buf()
    }

    fn canonicalize_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
        paths.iter().map(|p| Self::canonicalize_path(p)).collect()
    }

    /// Returns module names for all files transitively affected by the given changed paths.
    /// Used to reload Python modules in the worker pool.
    pub fn affected_modules(&self, changed: &[PathBuf]) -> Vec<String> {
        let changed = Self::canonicalize_paths(changed);
        let affected = self.import_graph.affected_files(&changed);
        let mut modules: Vec<String> = affected
            .iter()
            .map(|p| crate::path_to_module(&self.root, p))
            .collect();
        modules.sort();
        debug!(
            "affected_modules: {:?} → {:?}",
            changed
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            modules
        );
        modules
    }

    /// Returns only tests whose source file is transitively affected by the changed paths.
    pub fn tests_for_changed(&self, changed: &[PathBuf]) -> Vec<TestItem> {
        let changed = Self::canonicalize_paths(changed);
        let affected = self.import_graph.affected_files(&changed);
        let tests: Vec<TestItem> = self
            .tests()
            .into_iter()
            .filter(|t| {
                t.file_path
                    .as_ref()
                    .is_some_and(|rel| affected.contains(&self.root.join(rel)))
            })
            .collect();
        debug!(
            "tests_for_changed: {:?} → {} tests",
            changed
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            tests.len()
        );
        tests
    }

    /// Returns a sorted summary of the import graph for all known files.
    pub fn import_graph_summary(&self) -> Vec<GraphEntry> {
        let mut entries: Vec<GraphEntry> = self
            .inputs
            .keys()
            .map(|file| {
                let imports = self
                    .import_graph
                    .imports_for(file)
                    .into_iter()
                    .map(|p| p.strip_prefix(&self.root).unwrap_or(p).to_path_buf())
                    .collect();
                let imported_by = self
                    .import_graph
                    .imported_by_for(file)
                    .into_iter()
                    .map(|p| p.strip_prefix(&self.root).unwrap_or(p).to_path_buf())
                    .collect();
                GraphEntry {
                    file: file.strip_prefix(&self.root).unwrap_or(file).to_path_buf(),
                    imports,
                    imported_by,
                }
            })
            .collect();
        entries.sort_by(|a, b| a.file.cmp(&b.file));
        entries
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
    fn tests_returns_same_results_as_prior_rediscover() {
        let source = "@test\ndef test_hello():\n    pass\n";
        let dir = make_project(&[("test_example.py", source)]);
        let mut discoverer = Discoverer::new(dir.path());
        let mut from_rediscover = discoverer.rediscover();
        let mut from_tests = discoverer.tests();
        from_rediscover.sort_by(|a, b| a.name.cmp(&b.name));
        from_tests.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(from_rediscover.len(), from_tests.len());
        for (a, b) in from_rediscover.iter().zip(from_tests.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.module_path, b.module_path);
        }
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

    #[test]
    fn tests_for_changed_returns_only_affected_tests() {
        let utils_src = "def helper(): pass\n";
        let test_foo_src = "from utils import helper\n@test\ndef test_foo():\n    pass\n";
        let test_bar_src = "from utils import helper\n@test\ndef test_bar():\n    pass\n";
        let isolated_src = "@test\ndef test_baz():\n    pass\n";
        let dir = make_project(&[
            ("utils.py", utils_src),
            ("test_foo.py", test_foo_src),
            ("test_bar.py", test_bar_src),
            ("test_baz.py", isolated_src),
        ]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        let changed = vec![dir.path().join("utils.py")];
        let mut tests = discoverer.tests_for_changed(&changed);
        tests.sort_by(|a, b| a.name.cmp(&b.name));

        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"test_foo"), "test_foo should be affected");
        assert!(names.contains(&"test_bar"), "test_bar should be affected");
        assert!(
            !names.contains(&"test_baz"),
            "test_baz should not be affected"
        );
    }

    #[test]
    fn affected_modules_returns_module_names() {
        let utils_src = "def helper(): pass\n";
        let test_foo_src = "from utils import helper\n@test\ndef test_foo():\n    pass\n";
        let dir = make_project(&[("utils.py", utils_src), ("test_foo.py", test_foo_src)]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        let changed = vec![dir.path().join("utils.py")];
        let mut modules = discoverer.affected_modules(&changed);
        modules.sort();

        assert!(modules.contains(&"test_foo".to_string()));
        assert!(modules.contains(&"utils".to_string()));
    }

    #[test]
    fn import_graph_summary_shows_edges() {
        let utils_src = "def helper(): pass\n";
        let test_foo_src = "from utils import helper\n@test\ndef test_foo():\n    pass\n";
        let dir = make_project(&[("utils.py", utils_src), ("test_foo.py", test_foo_src)]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        let summary = discoverer.import_graph_summary();
        let utils_entry = summary
            .iter()
            .find(|e| e.file == Path::new("utils.py"))
            .expect("utils.py entry");
        assert!(utils_entry.imports.is_empty());
        assert!(
            utils_entry
                .imported_by
                .contains(&PathBuf::from("test_foo.py"))
        );

        let foo_entry = summary
            .iter()
            .find(|e| e.file == Path::new("test_foo.py"))
            .expect("test_foo.py entry");
        assert!(foo_entry.imports.contains(&PathBuf::from("utils.py")));
        assert!(foo_entry.imported_by.is_empty());
    }

    #[test]
    fn tests_for_changed_dotted_absolute_import() {
        let auth_src = "def login(): pass\n";
        let test_auth_src =
            "from src.services.auth import login\n@test\ndef test_login():\n    pass\n";
        let isolated_src = "@test\ndef test_other():\n    pass\n";
        let dir = make_project(&[
            ("src/__init__.py", ""),
            ("src/services/__init__.py", ""),
            ("src/services/auth.py", auth_src),
            ("tests/test_auth.py", test_auth_src),
            ("tests/test_other.py", isolated_src),
        ]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        let changed = vec![dir.path().join("src/services/auth.py")];
        let tests = discoverer.tests_for_changed(&changed);
        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"test_login"),
            "test_login should be affected by change to src/services/auth.py, got: {names:?}"
        );
        assert!(
            !names.contains(&"test_other"),
            "test_other should not be affected"
        );
    }

    #[test]
    fn tests_for_changed_canonical_vs_noncanonical_paths() {
        let auth_src = "def login(): pass\n";
        let test_auth_src =
            "from src.services.auth import login\n@test\ndef test_login():\n    pass\n";
        let dir = make_project(&[
            ("src/__init__.py", ""),
            ("src/services/__init__.py", ""),
            ("src/services/auth.py", auth_src),
            ("tests/test_auth.py", test_auth_src),
        ]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        // simulate canonical path (e.g. macOS /private/var vs /var, or watcher paths)
        let canonical = dir
            .path()
            .join("src/services/auth.py")
            .canonicalize()
            .expect("canonicalize");
        let tests = discoverer.tests_for_changed(&[canonical]);
        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"test_login"),
            "should find test_login even with canonical changed path, got: {names:?}"
        );
    }

    #[test]
    fn import_graph_summary_connected_only_filter() {
        let utils_src = "def helper(): pass\n";
        let test_foo_src = "from utils import helper\n@test\ndef test_foo():\n    pass\n";
        let isolated_src = "@test\ndef test_isolated():\n    pass\n";
        let dir = make_project(&[
            ("utils.py", utils_src),
            ("test_foo.py", test_foo_src),
            ("test_isolated.py", isolated_src),
        ]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        let summary = discoverer.import_graph_summary();
        let connected: Vec<_> = summary
            .iter()
            .filter(|e| !e.imports.is_empty() || !e.imported_by.is_empty())
            .collect();
        let files: Vec<&PathBuf> = connected.iter().map(|e| &e.file).collect();
        assert!(files.contains(&&PathBuf::from("utils.py")));
        assert!(files.contains(&&PathBuf::from("test_foo.py")));
        assert!(!files.contains(&&PathBuf::from("test_isolated.py")));
    }
}
