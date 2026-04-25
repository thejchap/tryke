use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use log::{debug, trace, warn};
use rayon::prelude::*;
use salsa::Setter;
use tryke_types::{HookItem, TestItem};

use crate::{
    cache::{DiskCache, FileKey},
    db::{Database, DiscoveredFile, SourceFile, discover_file},
    import_graph::{GraphEntry, ImportGraph},
};

pub struct Discoverer {
    db: Database,
    inputs: HashMap<PathBuf, SourceFile>,
    root: PathBuf,
    /// Absolute source roots used to resolve `from foo.bar import x`
    /// against the enumerated project files. Derived from
    /// `[tool.tryke] src` joined onto `root` and canonicalized. Defaults
    /// to `[root]` (project root) when the config doesn't set `src`.
    src_roots: Vec<PathBuf>,
    import_graph: ImportGraph,
    excludes: Vec<String>,
    /// Set of all project-local python files known to the discoverer.
    /// Populated by the most recent `rediscover` and updated by
    /// `rediscover_changed` so import candidates can be resolved via
    /// `HashSet` membership instead of per-import `stat()` syscalls.
    project_files: HashSet<PathBuf>,
    /// Authoritative store of per-file discovery results. Populated by
    /// the most recent `rediscover` / `rediscover_changed` from either
    /// a disk-cache hit or a salsa parse. Reader methods (`tests`,
    /// `hooks`, `testing_guard_else_locations`) read from here so
    /// cached files are visible without a salsa parse.
    results: HashMap<PathBuf, DiscoveredFile>,
    /// Persistent mtime/size-keyed cache of `DiscoveredFile` results,
    /// loaded at construction and saved after each `rediscover`.
    cache: DiskCache,
    /// The `FileKey` (mtime + size) observed during the most recent
    /// stat of each enumerated file. Used to decide which entries to
    /// persist back into `cache` after parsing.
    cache_keys_hit: HashMap<PathBuf, FileKey>,
}

/// Result of the parallel stat-and-maybe-read phase of `rediscover`.
enum FileWork {
    Hit {
        path: PathBuf,
        data: DiscoveredFile,
        key: FileKey,
    },
    Miss {
        path: PathBuf,
        source: String,
        key: FileKey,
    },
    StatError {
        path: PathBuf,
    },
}

impl Discoverer {
    #[must_use]
    pub fn new(start: &Path) -> Self {
        let config = tryke_config::load_effective_config(start);
        Self::new_with_options(start, &config.discovery.exclude, &config.discovery.src)
    }

    #[must_use]
    pub fn new_with_excludes(start: &Path, excludes: &[String]) -> Self {
        let src = tryke_config::load_effective_config(start).discovery.src;
        Self::new_with_options(start, excludes, &src)
    }

    #[must_use]
    pub fn new_with_options(start: &Path, excludes: &[String], src: &[String]) -> Self {
        let root = crate::find_project_root(start).unwrap_or_else(|| start.to_path_buf());
        let root = root.canonicalize().unwrap_or(root);
        let src_roots = if src.is_empty() {
            vec![root.clone()]
        } else {
            crate::resolve_src_roots(&root, src)
        };
        let cache_path = root.join(".tryke").join("cache").join("discovery-v1.bin");
        let cache = DiskCache::load(cache_path);
        Self {
            db: Database::default(),
            inputs: HashMap::new(),
            root,
            src_roots,
            import_graph: ImportGraph::default(),
            excludes: excludes.to_vec(),
            project_files: HashSet::new(),
            results: HashMap::new(),
            cache,
            cache_keys_hit: HashMap::new(),
        }
    }

    pub fn rediscover(&mut self) -> Vec<TestItem> {
        let mut paths = crate::collect_python_files(&self.root, &self.excludes);
        paths.sort();
        debug!(
            "rediscover: found {} python files in {}",
            paths.len(),
            self.root.display()
        );
        let path_set: HashSet<PathBuf> = paths.iter().cloned().collect();

        // Phase 1: in parallel, stat every file and consult the disk
        // cache. For cache hits we keep the cached `DiscoveredFile`;
        // for misses we also carry the file text (read opportunistically
        // during stat) so the parallel parse phase doesn't have to read
        // it again. The closure captures `&self.cache` only — the
        // rest of `self` holds a salsa `Database` that isn't `Sync`.
        let cache_ref = &self.cache;
        let keyed: Vec<FileWork> = paths
            .par_iter()
            .map(|path| Self::prepare_work(cache_ref, path))
            .collect();

        // Phase 2: drop cache entries for paths we no longer enumerate
        // (file deleted or newly excluded).
        self.cache.retain(&path_set);

        // Phase 3: serial ingest of cache hits + upsert misses into
        // salsa. Salsa mutations require `&mut self.db`, so this runs
        // single-threaded. The expensive work has already happened.
        let mut misses: Vec<PathBuf> = Vec::new();
        let mut hit_count = 0usize;
        let removed: Vec<PathBuf> = self
            .results
            .keys()
            .filter(|p| !path_set.contains(*p))
            .cloned()
            .collect();
        for path in removed {
            self.import_graph.remove(&path);
            self.inputs.remove(&path);
            self.results.remove(&path);
        }
        for work in keyed {
            match work {
                FileWork::Hit { path, data, key } => {
                    // Hot path: file unchanged since last run. Use the
                    // cached result directly; no parse, no salsa.
                    self.results.insert(path.clone(), data);
                    self.cache_keys_hit
                        .entry(path)
                        .and_modify(|k| *k = key)
                        .or_insert(key);
                    hit_count += 1;
                }
                FileWork::Miss { path, source, key } => {
                    self.upsert_source(&path, source);
                    self.cache_keys_hit.insert(path.clone(), key);
                    misses.push(path);
                }
                FileWork::StatError { path } => {
                    warn!("rediscover: stat failed for {}, skipping", path.display());
                }
            }
        }
        debug!(
            "rediscover: cache hits {}/{} ({} parses pending)",
            hit_count,
            paths.len(),
            misses.len()
        );

        // Phase 4: parallel parse for all misses. Salsa's memo tables
        // absorb concurrent queries via the cloned `StorageHandle`.
        let miss_snapshots: Vec<(PathBuf, SourceFile)> = misses
            .iter()
            .filter_map(|p| self.inputs.get(p).map(|f| (p.clone(), *f)))
            .collect();
        let miss_results: Vec<(PathBuf, DiscoveredFile)> = self.parse_in_parallel(&miss_snapshots);
        for (path, data) in &miss_results {
            self.results.insert(path.clone(), data.clone());
            if let Some(&key) = self.cache_keys_hit.get(path) {
                self.cache.insert(path.clone(), key, data.clone());
            }
        }

        // Phase 5: resolve import candidates for every file (hit + miss)
        // against the enumerated file set, then update the import
        // graph and collect tests. Resolution is parallel — per-file
        // work is independent and fast HashSet lookups still accrue.
        let resolved: Vec<(PathBuf, Vec<PathBuf>, bool, Vec<TestItem>)> = self
            .results
            .par_iter()
            .map(|(path, result)| {
                let imports =
                    crate::resolve_import_candidate_groups(&result.import_candidates, &path_set);
                (
                    path.clone(),
                    imports,
                    result.dynamic_imports,
                    result.parsed.tests.clone(),
                )
            })
            .collect();
        let mut tests: Vec<TestItem> = Vec::new();
        for (path, imports, dynamic, file_tests) in resolved {
            self.import_graph.update(path.clone(), imports);
            if dynamic {
                self.import_graph.mark_always_dirty(path);
            } else {
                self.import_graph.clear_always_dirty(&path);
            }
            tests.extend(file_tests);
        }
        self.project_files = path_set;

        // Phase 6: persist the cache. Save errors are logged but not
        // propagated — a stale cache is annoying, but a failed save
        // shouldn't fail discovery.
        if let Err(err) = self.cache.save() {
            warn!("rediscover: failed to save discovery cache: {err}");
        }

        debug!("rediscover: discovered {} tests total", tests.len());
        tests
    }

    /// Stat `path` and consult the disk cache. On a cache hit, return
    /// the cached `DiscoveredFile` without reading the file. On a
    /// miss, read the file text so the parse phase doesn't stat-then-
    /// read (halving the syscalls per miss).
    ///
    /// Takes `&DiskCache` rather than `&self` so rayon workers can
    /// share it across threads — `Discoverer` holds a salsa
    /// `Database` that isn't `Sync`.
    fn prepare_work(cache: &DiskCache, path: &Path) -> FileWork {
        let Ok(metadata) = std::fs::metadata(path) else {
            return FileWork::StatError {
                path: path.to_path_buf(),
            };
        };
        let Ok(key) = FileKey::from_metadata(&metadata) else {
            return FileWork::StatError {
                path: path.to_path_buf(),
            };
        };
        if let Some(data) = cache.get(path, &key) {
            FileWork::Hit {
                path: path.to_path_buf(),
                data: data.clone(),
                key,
            }
        } else {
            let source = std::fs::read_to_string(path).unwrap_or_default();
            FileWork::Miss {
                path: path.to_path_buf(),
                source,
                key,
            }
        }
    }

    /// Run `discover_file` across many salsa `SourceFile` inputs in parallel.
    /// Each rayon worker materialises its own `Database` from a `Sync`
    /// `StorageHandle`; the underlying salsa memo tables are shared via Arc,
    /// so parses populate one cache.
    fn parse_in_parallel(
        &self,
        snapshots: &[(PathBuf, SourceFile)],
    ) -> Vec<(PathBuf, DiscoveredFile)> {
        let handle = self.db.storage_handle();
        snapshots
            .par_iter()
            .map_init(
                || Database::from_handle(handle.clone()),
                |db, (path, file)| (path.clone(), discover_file(db, *file)),
            )
            .collect()
    }

    /// Upsert the salsa input for `path` with the given text: either create
    /// a new `SourceFile` or call `set_text` on the existing one if changed.
    fn upsert_source(&mut self, path: &Path, text: String) {
        if let Some(file) = self.inputs.get(path) {
            if file.text(&self.db) != &text {
                trace!("rediscover: re-parsing changed file {}", path.display());
                file.set_text(&mut self.db).to(text);
            }
        } else {
            trace!("rediscover: parsing new file {}", path.display());
            let file = SourceFile::new(
                &self.db,
                text,
                self.root.clone(),
                self.src_roots.clone(),
                path.to_path_buf(),
            );
            self.inputs.insert(path.to_path_buf(), file);
        }
    }

    pub fn tests(&self) -> Vec<TestItem> {
        self.results
            .values()
            .flat_map(|r| r.parsed.tests.clone())
            .collect()
    }

    /// Returns all hooks discovered across all known files.
    pub fn hooks(&self) -> Vec<HookItem> {
        self.results
            .values()
            .flat_map(|r| r.parsed.hooks.clone())
            .collect()
    }

    pub fn rediscover_changed(&mut self, changed: &[PathBuf]) -> Vec<TestItem> {
        let changed = Self::canonicalize_paths(changed);
        debug!(
            "rediscover_changed: processing {} changed paths",
            changed.len()
        );
        let mut touched: Vec<PathBuf> = Vec::new();
        for path in &changed {
            if path.extension().is_some_and(|ext| ext == "py") {
                if path.exists() {
                    let text = std::fs::read_to_string(path).unwrap_or_default();
                    self.upsert_source(path, text);
                    self.project_files.insert(path.clone());
                    touched.push(path.clone());
                } else {
                    trace!(
                        "rediscover_changed: removing deleted file {}",
                        path.display()
                    );
                    self.import_graph.remove(path);
                    self.inputs.remove(path);
                    self.project_files.remove(path);
                    self.results.remove(path);
                    self.cache.remove(path);
                }
            }
        }
        for path in &touched {
            if let Some(file) = self.inputs.get(path).copied() {
                let result = discover_file(&self.db, file);
                let imports = crate::resolve_import_candidate_groups(
                    &result.import_candidates,
                    &self.project_files,
                );
                self.import_graph.update(path.clone(), imports);
                if result.dynamic_imports {
                    self.import_graph.mark_always_dirty(path.clone());
                } else {
                    self.import_graph.clear_always_dirty(path);
                }
                // Update both the in-memory result set and the disk
                // cache so the new parse sticks for this file.
                if let Ok(key) = FileKey::from_path(path) {
                    self.cache.insert(path.clone(), key, result.clone());
                }
                self.results.insert(path.clone(), result);
            }
        }
        if let Err(err) = self.cache.save() {
            warn!("rediscover_changed: failed to save discovery cache: {err}");
        }
        let tests: Vec<TestItem> = self
            .results
            .values()
            .flat_map(|r| r.parsed.tests.clone())
            .collect();
        debug!("rediscover_changed: {} tests after update", tests.len());
        tests
    }

    fn canonicalize_path(p: &Path) -> PathBuf {
        if let Ok(c) = p.canonicalize() {
            return c;
        }
        // File may be deleted; canonicalize parent + filename
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

    /// Returns all files transitively affected by the given changed paths.
    pub fn affected_files(&self, changed: &[PathBuf]) -> HashSet<PathBuf> {
        let changed = Self::canonicalize_paths(changed);
        self.import_graph.affected_files(&changed)
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
        let affected = self.affected_files(changed);
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
            Self::canonicalize_paths(changed)
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            tests.len()
        );
        tests
    }

    /// Returns all files that contain dynamic imports (`importlib.import_module()` or
    /// `__import__()`). These files are marked always-dirty: they are included in every
    /// `--changed` run and may produce stale module state in watch/server mode.
    pub fn dynamic_import_files(&self) -> Vec<PathBuf> {
        self.import_graph.always_dirty_files()
    }

    /// Returns `(file, line)` pairs for every `if __TRYKE_TESTING__:` statement
    /// that has an `elif` or `else` branch. Discovery silently drops these
    /// guards; the caller surfaces them as warnings so users don't debug
    /// missing tests from an unsupported guard shape.
    pub fn testing_guard_else_locations(&self) -> Vec<(PathBuf, u32)> {
        let mut lines: Vec<(PathBuf, u32)> = Vec::new();
        for (path, result) in &self.results {
            for line in &result.parsed.testing_guard_else_lines {
                lines.push((path.clone(), *line));
            }
        }
        lines.sort();
        lines
    }

    /// Returns a sorted summary of the import graph for all known files.
    pub fn import_graph_summary(&self) -> Vec<GraphEntry> {
        let mut entries: Vec<GraphEntry> = self
            .results
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

        // Modify both files on disk, but only notify about test_a
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
    fn tests_for_changed_absolute_imported_submodule() {
        let dir = make_project(&[
            ("pkg/__init__.py", ""),
            ("pkg/helpers.py", "def helper(): pass\n"),
            (
                "tests/test_helpers.py",
                "from pkg import helpers\n@test\ndef test_helper_user():\n    pass\n",
            ),
            (
                "tests/test_other.py",
                "@test\ndef test_other():\n    pass\n",
            ),
        ]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        let changed = vec![dir.path().join("pkg/helpers.py")];
        let tests = discoverer.tests_for_changed(&changed);
        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"test_helper_user"),
            "test_helper_user should be affected by change to pkg/helpers.py, got: {names:?}"
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

        // Simulate canonical path (e.g. macOS /private/var vs /var, or watcher paths)
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

    #[test]
    fn dynamic_import_file_always_included_in_tests_for_changed() {
        let dynamic_src = "import importlib\nmod = importlib.import_module('utils')\n@test\ndef test_dyn():\n    pass\n";
        let static_src = "@test\ndef test_static():\n    pass\n";
        let utils_src = "def helper(): pass\n";
        let dir = make_project(&[
            ("test_dynamic.py", dynamic_src),
            ("test_static.py", static_src),
            ("utils.py", utils_src),
        ]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        // Change only utils.py — test_dynamic should be included because it has dynamic imports
        let changed = vec![dir.path().join("utils.py")];
        let tests = discoverer.tests_for_changed(&changed);
        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"test_dyn"),
            "test_dyn should be included (always-dirty), got: {names:?}"
        );
        assert!(
            !names.contains(&"test_static"),
            "test_static should not be affected"
        );
    }

    #[test]
    fn dynamic_import_files_reflects_always_dirty() {
        let dynamic_src = "import importlib\nmod = importlib.import_module('utils')\nfrom tryke import test\n@test\ndef test_dyn():\n    pass\n";
        let static_src = "from tryke import test\n@test\ndef test_static():\n    pass\n";
        let dir = make_project(&[
            ("test_dynamic.py", dynamic_src),
            ("test_static.py", static_src),
        ]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        let files = discoverer.dynamic_import_files();
        let names: Vec<&str> = files
            .iter()
            .filter_map(|p| p.file_name())
            .filter_map(|n| n.to_str())
            .collect();
        assert!(
            names.contains(&"test_dynamic.py"),
            "test_dynamic.py should be in dynamic_import_files, got: {names:?}"
        );
        assert!(
            !names.contains(&"test_static.py"),
            "test_static.py should not be in dynamic_import_files"
        );
    }

    #[test]
    fn dynamic_import_cleared_when_removed_from_source() {
        let dynamic_src = "import importlib\nmod = importlib.import_module('foo')\n@test\ndef test_dyn():\n    pass\n";
        let dir = make_project(&[("test_dynamic.py", dynamic_src)]);
        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        // Rewrite without dynamic import
        let static_src = "@test\ndef test_dyn():\n    pass\n";
        fs::write(dir.path().join("test_dynamic.py"), static_src).expect("write");
        discoverer.rediscover_changed(&[dir.path().join("test_dynamic.py")]);

        // Now changing an unrelated file should NOT include test_dynamic
        let changed = vec![dir.path().join("unrelated.py")];
        let tests = discoverer.tests_for_changed(&changed);
        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !names.contains(&"test_dyn"),
            "test_dyn should no longer be always-dirty, got: {names:?}"
        );
    }

    #[test]
    fn absolute_import_resolves_under_configured_src_root() {
        // Mirrors the real-world tryke layout: package under `python/` is
        // a top-level import root, but the test file that imports it
        // lives at the repo root alongside `tests/`. Without `src =
        // ["python"]` in the config, `from mypkg.mod import X` would
        // resolve to `<root>/mypkg/mod.py` — which doesn't exist — and
        // the import graph edge would be dropped.
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.tryke]\nsrc = [\".\", \"python\"]\n",
        )
        .expect("write pyproject");
        fs::create_dir_all(dir.path().join("python/mypkg")).expect("mkdir mypkg");
        fs::create_dir_all(dir.path().join("tests")).expect("mkdir tests");
        fs::write(
            dir.path().join("python/mypkg/__init__.py"),
            "# package marker\n",
        )
        .expect("write __init__.py");
        fs::write(
            dir.path().join("python/mypkg/mod.py"),
            "def value() -> int:\n    return 1\n",
        )
        .expect("write mod.py");
        fs::write(
            dir.path().join("tests/test_mod.py"),
            "from mypkg.mod import value\n\n@test\ndef test_value():\n    assert value() == 1\n",
        )
        .expect("write test_mod.py");

        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        let entries = discoverer.import_graph_summary();
        let test_entry = entries
            .iter()
            .find(|e| e.file == Path::new("tests/test_mod.py"))
            .expect("tests/test_mod.py in graph");
        assert!(
            test_entry
                .imports
                .iter()
                .any(|p| p == Path::new("python/mypkg/mod.py")),
            "test_mod.py should import python/mypkg/mod.py via src=[\"python\"]; got {:?}",
            test_entry.imports
        );
    }

    #[test]
    fn absolute_import_without_src_root_does_not_resolve() {
        // Default `src = ["."]` means `from mypkg.mod import X` only
        // looks under the project root. The file lives under python/
        // so the import edge is dropped — this is the pre-src behavior
        // we want to preserve for projects without a python-source layout.
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject");
        fs::create_dir_all(dir.path().join("python/mypkg")).expect("mkdir mypkg");
        fs::write(
            dir.path().join("python/mypkg/__init__.py"),
            "# package marker\n",
        )
        .expect("write __init__.py");
        fs::write(
            dir.path().join("python/mypkg/mod.py"),
            "def value() -> int:\n    return 1\n",
        )
        .expect("write mod.py");
        fs::write(
            dir.path().join("test_mod.py"),
            "from mypkg.mod import value\n\n@test\ndef test_value():\n    assert value() == 1\n",
        )
        .expect("write test_mod.py");

        let mut discoverer = Discoverer::new(dir.path());
        discoverer.rediscover();

        let entries = discoverer.import_graph_summary();
        let test_entry = entries
            .iter()
            .find(|e| e.file == Path::new("test_mod.py"))
            .expect("test_mod.py in graph");
        assert!(
            test_entry.imports.is_empty(),
            "without src=[\"python\"], mypkg.mod should not resolve; got {:?}",
            test_entry.imports
        );
    }
}
