use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

use log::trace;

#[derive(Default)]
pub struct ImportGraph {
    forward: HashMap<PathBuf, HashSet<PathBuf>>,
    reverse: HashMap<PathBuf, HashSet<PathBuf>>,
    always_dirty: HashSet<PathBuf>,
}

pub struct GraphEntry {
    pub file: PathBuf,
    pub imports: Vec<PathBuf>,
    pub imported_by: Vec<PathBuf>,
}

impl ImportGraph {
    pub fn update(&mut self, file: PathBuf, imports: Vec<PathBuf>) {
        // Remove old forward edges from reverse index
        if let Some(old_imports) = self.forward.remove(&file) {
            for old_import in &old_imports {
                if let Some(importers) = self.reverse.get_mut(old_import) {
                    importers.remove(&file);
                }
            }
        }
        // Build new forward edge set and update reverse index
        let new_set: HashSet<PathBuf> = imports.into_iter().collect();
        for import in &new_set {
            self.reverse
                .entry(import.clone())
                .or_default()
                .insert(file.clone());
        }
        self.forward.insert(file, new_set);
    }

    pub fn remove(&mut self, file: &Path) {
        if let Some(old_imports) = self.forward.remove(file) {
            for old_import in &old_imports {
                if let Some(importers) = self.reverse.get_mut(old_import) {
                    importers.remove(file);
                }
            }
        }
        self.always_dirty.remove(file);
        // Keep reverse[file] intact so affected_files can still find importers of a deleted file
    }

    pub fn mark_always_dirty(&mut self, file: PathBuf) {
        self.always_dirty.insert(file);
    }

    pub fn clear_always_dirty(&mut self, file: &Path) {
        self.always_dirty.remove(file);
    }

    /// BFS over the reverse index: returns all files that transitively depend on `changed`.
    /// Includes the changed files themselves and any always-dirty files.
    pub fn affected_files(&self, changed: &[PathBuf]) -> HashSet<PathBuf> {
        let mut visited: HashSet<PathBuf> = HashSet::new();
        let mut queue: VecDeque<PathBuf> = VecDeque::new();

        for file in changed {
            if visited.insert(file.clone()) {
                queue.push_back(file.clone());
            }
        }

        while let Some(file) = queue.pop_front() {
            if let Some(importers) = self.reverse.get(&file) {
                for importer in importers {
                    if visited.insert(importer.clone()) {
                        trace!(
                            "import_graph: {} invalidated by change to {}",
                            importer.display(),
                            file.display()
                        );
                        queue.push_back(importer.clone());
                    }
                }
            }
        }

        visited.extend(self.always_dirty.iter().cloned());
        visited
    }

    pub fn imports_for(&self, file: &Path) -> Vec<&PathBuf> {
        let mut imports: Vec<&PathBuf> = self
            .forward
            .get(file)
            .map(|s| s.iter().collect())
            .unwrap_or_default();
        imports.sort();
        imports
    }

    pub fn imported_by_for(&self, file: &Path) -> Vec<&PathBuf> {
        let mut importers: Vec<&PathBuf> = self
            .reverse
            .get(file)
            .map(|s| s.iter().collect())
            .unwrap_or_default();
        importers.sort();
        importers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn update_adds_forward_and_reverse_edges() {
        let mut g = ImportGraph::default();
        g.update(p("/proj/test_foo.py"), vec![p("/proj/utils.py")]);
        assert!(g.forward[&p("/proj/test_foo.py")].contains(&p("/proj/utils.py")));
        assert!(g.reverse[&p("/proj/utils.py")].contains(&p("/proj/test_foo.py")));
    }

    #[test]
    fn update_replaces_old_edges() {
        let mut g = ImportGraph::default();
        g.update(p("/proj/test_foo.py"), vec![p("/proj/old.py")]);
        g.update(p("/proj/test_foo.py"), vec![p("/proj/new.py")]);
        assert!(!g.forward[&p("/proj/test_foo.py")].contains(&p("/proj/old.py")));
        assert!(
            !g.reverse
                .get(&p("/proj/old.py"))
                .is_some_and(|s| s.contains(&p("/proj/test_foo.py")))
        );
        assert!(g.forward[&p("/proj/test_foo.py")].contains(&p("/proj/new.py")));
        assert!(g.reverse[&p("/proj/new.py")].contains(&p("/proj/test_foo.py")));
    }

    #[test]
    fn remove_clears_forward_and_updates_reverse() {
        let mut g = ImportGraph::default();
        g.update(p("/proj/test_foo.py"), vec![p("/proj/utils.py")]);
        g.remove(&p("/proj/test_foo.py"));
        assert!(!g.forward.contains_key(&p("/proj/test_foo.py")));
        assert!(
            !g.reverse
                .get(&p("/proj/utils.py"))
                .is_some_and(|s| s.contains(&p("/proj/test_foo.py")))
        );
    }

    #[test]
    fn affected_files_includes_changed_and_importers() {
        let mut g = ImportGraph::default();
        g.update(p("/proj/test_foo.py"), vec![p("/proj/utils.py")]);
        g.update(p("/proj/test_bar.py"), vec![p("/proj/utils.py")]);

        let affected = g.affected_files(&[p("/proj/utils.py")]);
        assert!(affected.contains(&p("/proj/utils.py")));
        assert!(affected.contains(&p("/proj/test_foo.py")));
        assert!(affected.contains(&p("/proj/test_bar.py")));
    }

    #[test]
    fn affected_files_transitive() {
        let mut g = ImportGraph::default();
        // a <- b <- c (c imports b which imports a)
        g.update(p("/proj/b.py"), vec![p("/proj/a.py")]);
        g.update(p("/proj/c.py"), vec![p("/proj/b.py")]);

        let affected = g.affected_files(&[p("/proj/a.py")]);
        assert!(affected.contains(&p("/proj/a.py")));
        assert!(affected.contains(&p("/proj/b.py")));
        assert!(affected.contains(&p("/proj/c.py")));
    }

    #[test]
    fn affected_files_no_dependents() {
        let mut g = ImportGraph::default();
        g.update(p("/proj/test_foo.py"), vec![p("/proj/utils.py")]);

        let affected = g.affected_files(&[p("/proj/test_foo.py")]);
        assert_eq!(affected.len(), 1);
        assert!(affected.contains(&p("/proj/test_foo.py")));
    }

    #[test]
    fn imports_for_returns_sorted() {
        let mut g = ImportGraph::default();
        g.update(
            p("/proj/a.py"),
            vec![p("/proj/z.py"), p("/proj/m.py"), p("/proj/b.py")],
        );
        let imports = g.imports_for(&p("/proj/a.py"));
        let mut sorted = imports.clone();
        sorted.sort();
        assert_eq!(imports, sorted);
    }

    #[test]
    fn imported_by_for_returns_sorted() {
        let mut g = ImportGraph::default();
        g.update(p("/proj/z.py"), vec![p("/proj/utils.py")]);
        g.update(p("/proj/a.py"), vec![p("/proj/utils.py")]);
        g.update(p("/proj/m.py"), vec![p("/proj/utils.py")]);
        let imported_by = g.imported_by_for(&p("/proj/utils.py"));
        let mut sorted = imported_by.clone();
        sorted.sort();
        assert_eq!(imported_by, sorted);
    }

    #[test]
    fn remove_keeps_reverse_for_deleted_file() {
        // After removing utils.py, reverse[utils.py] should still hold test_foo.py
        // so that affected_files([utils.py]) can find test_foo.py
        let mut g = ImportGraph::default();
        g.update(p("/proj/test_foo.py"), vec![p("/proj/utils.py")]);
        g.remove(&p("/proj/utils.py"));
        // utils.py has no forward edges (it never had any), reverse is unchanged
        let affected = g.affected_files(&[p("/proj/utils.py")]);
        assert!(affected.contains(&p("/proj/test_foo.py")));
    }

    #[test]
    fn always_dirty_included_in_affected_files() {
        let mut g = ImportGraph::default();
        g.update(p("/proj/test_foo.py"), vec![]);
        g.mark_always_dirty(p("/proj/test_dyn.py"));

        let affected = g.affected_files(&[p("/proj/utils.py")]);
        assert!(affected.contains(&p("/proj/test_dyn.py")));
        assert!(affected.contains(&p("/proj/utils.py")));
    }

    #[test]
    fn clear_always_dirty_removes_file() {
        let mut g = ImportGraph::default();
        g.mark_always_dirty(p("/proj/test_dyn.py"));
        g.clear_always_dirty(&p("/proj/test_dyn.py"));

        let affected = g.affected_files(&[p("/proj/utils.py")]);
        assert!(!affected.contains(&p("/proj/test_dyn.py")));
    }

    #[test]
    fn remove_clears_always_dirty() {
        let mut g = ImportGraph::default();
        g.update(p("/proj/test_dyn.py"), vec![]);
        g.mark_always_dirty(p("/proj/test_dyn.py"));
        g.remove(&p("/proj/test_dyn.py"));

        let affected = g.affected_files(&[p("/proj/other.py")]);
        assert!(!affected.contains(&p("/proj/test_dyn.py")));
    }
}
