//! Persistent on-disk cache of `DiscoveredFile` results keyed by
//! `(mtime_nanos, size)`. Loaded at `Discoverer` construction and
//! consulted in `rediscover` to skip parsing for unchanged files.
//!
//! The cache format is bumped via `CACHE_VERSION` on schema changes; a
//! mismatched version produces an empty cache on load. Writes are
//! atomic (write-to-temp + rename) so a crash can't corrupt the file.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use log::{debug, trace};
use serde::{Deserialize, Serialize};

use super::db::DiscoveredFile;

/// Bumped whenever the cache schema (the shape of `DiscoveredFile` or
/// its transitive fields) changes in a way that'd make old entries
/// invalid. A mismatch on load yields an empty cache.
///
/// v2: switched from `rmp_serde::to_vec` (structs-as-arrays) to
/// `to_vec_named` (structs-as-maps). The array encoding mis-deserialises
/// whenever a field uses `#[serde(skip_serializing_if = ...)]` —
/// `TestItem` alone skips nine optional fields, so every cached entry
/// was unreadable.
/// v3: absolute-import resolution now walks configured `src` roots,
/// so cached `import_candidates` from v2 (always keyed to project
/// root) would miss resolutions under secondary roots like `python/`.
const CACHE_VERSION: u32 = 3;

/// Name of the cache file within its directory. The stem is also reused
/// (with a `.tmp` extension) for the atomic write in `save`.
pub const CACHE_FILE_NAME: &str = "discovery-v1.bin";

/// Identity of a source file derived from `stat`. Cheap to obtain
/// without reading the file contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileKey {
    pub mtime_nanos: i128,
    pub size: u64,
}

impl FileKey {
    pub fn from_metadata(metadata: &fs::Metadata) -> io::Result<Self> {
        let mtime = metadata.modified()?;
        let mtime_nanos = match mtime.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
            Err(e) => -i128::try_from(e.duration().as_nanos()).unwrap_or(i128::MAX),
        };
        Ok(Self {
            mtime_nanos,
            size: metadata.len(),
        })
    }

    pub fn from_path(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Self::from_metadata(&metadata)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    key: FileKey,
    data: DiscoveredFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: HashMap<PathBuf, CacheEntry>,
}

#[derive(Debug, Default)]
pub struct DiskCache {
    entries: HashMap<PathBuf, CacheEntry>,
    /// The path we loaded from / will save to. `None` disables I/O
    /// (used by tests that don't want a filesystem footprint).
    path: Option<PathBuf>,
    /// Best-effort `.gitignore` to create if one does not exist.
    gitignore: Option<GitignoreConfig>,
}

#[derive(Debug)]
struct GitignoreConfig {
    dir: PathBuf,
    contents: &'static str,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanCacheReport {
    pub cache_dir: PathBuf,
    pub removed_entries: usize,
}

/// Remove tryke's persistent discovery cache for `start`.
///
/// When `cache_dir` is `None`, the default cache lives under tryke's owned
/// state directory at `<project-root>/.tryke/cache`, so the whole cache
/// directory can be removed. `start` is resolved the same way discovery finds a
/// project root: walk up to the nearest `pyproject.toml`, then fall back to
/// `start` when no project file exists. When a custom cache directory is
/// configured, only tryke-owned cache files are removed to avoid deleting
/// unrelated user data.
///
/// # Errors
///
/// Returns any filesystem error encountered while deleting the cache directory
/// or cache files, except missing cache paths which are treated as already
/// clean.
pub fn clean_project_cache(start: &Path, cache_dir: Option<&Path>) -> io::Result<CleanCacheReport> {
    match cache_dir {
        Some(cache_dir) => clean_custom_cache_dir(cache_dir),
        None => clean_default_cache_dir(start),
    }
}

fn clean_default_cache_dir(start: &Path) -> io::Result<CleanCacheReport> {
    let root = super::find_project_root(start).unwrap_or_else(|| start.to_path_buf());
    let root = root.canonicalize().unwrap_or(root);
    let cache_dir = root.join(".tryke").join("cache");
    let removed_entries = match fs::remove_dir_all(&cache_dir) {
        Ok(()) => 1,
        Err(err) if err.kind() == io::ErrorKind::NotFound => 0,
        Err(err) => return Err(err),
    };
    Ok(CleanCacheReport {
        cache_dir,
        removed_entries,
    })
}

fn clean_custom_cache_dir(cache_dir: &Path) -> io::Result<CleanCacheReport> {
    let cache_file = cache_dir.join(CACHE_FILE_NAME);
    let tmp_file = cache_file.with_extension("tmp");
    let mut removed_entries = 0;
    for path in [&cache_file, &tmp_file] {
        match fs::remove_file(path) {
            Ok(()) => removed_entries += 1,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(CleanCacheReport {
        cache_dir: cache_dir.to_path_buf(),
        removed_entries,
    })
}

impl DiskCache {
    /// Load a cache from an explicit `path`, writing no `.gitignore`.
    ///
    /// Test-only: production code must pick a gitignore policy via
    /// `load_in_state_dir` (broad) or `load_in_dir` (narrow), so the cache
    /// file's directory layout is always known to the constructor.
    #[cfg(test)]
    pub fn load(path: PathBuf) -> Self {
        Self::load_with_gitignore(path, None)
    }

    /// Load the discovery cache from the default `.tryke` state directory
    /// layout: the cache file lives at `state_dir/cache/<CACHE_FILE_NAME>`.
    ///
    /// `state_dir` receives a broad `*` `.gitignore`. That is safe — and
    /// future-proof for anything else tryke writes under it — because the
    /// state directory is created and exclusively owned by tryke.
    pub fn load_in_state_dir(state_dir: PathBuf) -> Self {
        let path = state_dir.join("cache").join(CACHE_FILE_NAME);
        Self::load_with_gitignore(
            path,
            Some(GitignoreConfig {
                dir: state_dir,
                contents: "# created by tryke\n*\n",
            }),
        )
    }

    /// Load the standard discovery cache file inside `cache_dir`.
    ///
    /// `cache_dir` is also the directory that receives the best-effort
    /// `.gitignore`, matching user intent for a custom cache location. Unlike
    /// the default `.tryke` state directory, this writes narrow patterns only:
    /// a user-provided cache directory may be an existing project directory.
    /// The `.gitignore` ignores itself too, so it doesn't surface as an
    /// untracked file when the cache dir lives inside a git repo.
    pub fn load_in_dir(cache_dir: PathBuf) -> Self {
        let path = cache_dir.join(CACHE_FILE_NAME);
        Self::load_with_gitignore(
            path,
            Some(GitignoreConfig {
                dir: cache_dir,
                contents: "# created by tryke\n/.gitignore\n/discovery-v1.bin\n/discovery-v1.tmp\n",
            }),
        )
    }

    fn load_with_gitignore(path: PathBuf, gitignore: Option<GitignoreConfig>) -> Self {
        let entries = match Self::try_load(&path) {
            Ok(entries) => entries,
            Err(err) => {
                trace!("discovery cache load failed ({err}): starting empty");
                HashMap::new()
            }
        };
        debug!(
            "discovery cache loaded {} entries from {}",
            entries.len(),
            path.display()
        );
        Self {
            entries,
            path: Some(path),
            gitignore,
        }
    }

    fn try_load(path: &Path) -> io::Result<HashMap<PathBuf, CacheEntry>> {
        let bytes = fs::read(path)?;
        let file: CacheFile = rmp_serde::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if file.version != CACHE_VERSION {
            debug!(
                "discovery cache version mismatch ({} vs {}): discarding",
                file.version, CACHE_VERSION
            );
            return Ok(HashMap::new());
        }
        Ok(file.entries)
    }

    /// Lookup a cached result for `path`. Returns the stored
    /// `DiscoveredFile` only if the recorded `FileKey` matches the
    /// current one (i.e. the file hasn't been modified).
    pub fn get(&self, path: &Path, current_key: &FileKey) -> Option<&DiscoveredFile> {
        self.entries
            .get(path)
            .filter(|e| &e.key == current_key)
            .map(|e| &e.data)
    }

    pub fn insert(&mut self, path: PathBuf, key: FileKey, data: DiscoveredFile) {
        self.entries.insert(path, CacheEntry { key, data });
    }

    pub fn remove(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    /// Drop cache entries whose paths are not in `keep`. Called after
    /// a full rediscover to prune entries for deleted / excluded files.
    pub fn retain(&mut self, keep: &std::collections::HashSet<PathBuf>) {
        self.entries.retain(|p, _| keep.contains(p));
    }

    /// Persist the cache to disk atomically via a temp file + rename.
    /// No-op if this cache was not constructed with a backing path.
    pub fn save(&self) -> io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Drop a `.gitignore` at the tryke state/cache directory so users
        // don't have to remember to ignore tryke's internal cache. Best-effort
        // — a write failure here shouldn't abort the save.
        if let Some(config) = self.gitignore.as_ref() {
            let _ = fs::create_dir_all(&config.dir);
            let gitignore = config.dir.join(".gitignore");
            if !gitignore.exists() {
                let _ = fs::write(&gitignore, config.contents);
            }
        }
        let file = CacheFile {
            version: CACHE_VERSION,
            entries: self.entries.clone(),
        };
        // `to_vec_named` encodes structs as maps (field names + values)
        // so `#[serde(skip_serializing_if = ...)]` + `#[serde(default)]`
        // round-trip: missing fields fill from `Default` on load
        // instead of corrupting positional alignment in the tuple form.
        let bytes = rmp_serde::to_vec_named(&file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &bytes)?;
        fs::rename(&tmp_path, path)?;
        debug!(
            "discovery cache saved {} entries to {}",
            self.entries.len(),
            path.display()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Nest the cache file so both its parent and grandparent are
        // inside the controlled tempdir — that lets us assert `load`
        // writes no `.gitignore` into any ancestor.
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested).expect("create nested");
        let path = nested.join("cache.json");
        let cache = DiskCache::load(path.clone());
        assert_eq!(cache.entries.len(), 0);
        cache.save().expect("save");
        let reloaded = DiskCache::load(path);
        assert_eq!(reloaded.entries.len(), 0);
        // `load` writes no `.gitignore` — not beside the cache file, and
        // crucially not in any ancestor directory.
        assert!(!nested.join(".gitignore").exists());
        assert!(!dir.path().join("a").join(".gitignore").exists());
        assert!(!dir.path().join(".gitignore").exists());
    }

    #[test]
    fn default_cache_writes_broad_gitignore_in_state_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_dir = dir.path().join(".tryke");
        let cache = DiskCache::load_in_state_dir(state_dir.clone());

        cache.save().expect("save");

        assert!(state_dir.join("cache").join("discovery-v1.bin").exists());
        let gitignore = fs::read_to_string(state_dir.join(".gitignore")).expect("read gitignore");
        assert_eq!(gitignore, "# created by tryke\n*\n");
    }

    #[test]
    fn custom_cache_dir_writes_narrow_gitignore() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache_dir = dir.path().join("custom-cache");
        let cache = DiskCache::load_in_dir(cache_dir.clone());

        cache.save().expect("save");

        let gitignore = fs::read_to_string(cache_dir.join(".gitignore")).expect("read gitignore");
        assert_eq!(
            gitignore,
            "# created by tryke\n/.gitignore\n/discovery-v1.bin\n/discovery-v1.tmp\n"
        );
        assert!(!gitignore.lines().any(|line| line.trim() == "*"));
        // The `.gitignore` ignores itself so it doesn't show up as untracked.
        assert!(gitignore.lines().any(|line| line.trim() == "/.gitignore"));
    }

    #[test]
    fn clean_default_cache_removes_owned_state_cache_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_dir = dir
            .path()
            .canonicalize()
            .expect("canonical tempdir")
            .join(".tryke");
        let cache_dir = state_dir.join("cache");
        fs::create_dir_all(&cache_dir).expect("create cache dir");
        fs::write(cache_dir.join("discovery-v1.bin"), b"cache").expect("write cache");
        fs::write(cache_dir.join("future-cache.bin"), b"future").expect("write future cache");
        fs::write(state_dir.join(".gitignore"), b"# created by tryke\n*\n")
            .expect("write gitignore");

        let report = clean_project_cache(dir.path(), None).expect("clean cache");

        assert_eq!(report.cache_dir, cache_dir);
        assert_eq!(report.removed_entries, 1);
        assert!(!report.cache_dir.exists());
        assert!(state_dir.join(".gitignore").exists());
    }

    #[test]
    fn clean_default_cache_resolves_project_root_from_subdir() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("pyproject.toml"),
            b"[project]\nname = \"sample\"\n",
        )
        .expect("write pyproject");
        let subdir = dir.path().join("src").join("pkg");
        fs::create_dir_all(&subdir).expect("create subdir");
        let project_root = dir.path().canonicalize().expect("canonical tempdir");
        let cache_dir = project_root.join(".tryke").join("cache");
        fs::create_dir_all(&cache_dir).expect("create cache dir");
        fs::write(cache_dir.join("discovery-v1.bin"), b"cache").expect("write cache");

        let report = clean_project_cache(&subdir, None).expect("clean cache");

        assert_eq!(report.cache_dir, cache_dir);
        assert_eq!(report.removed_entries, 1);
        assert!(!report.cache_dir.exists());
    }

    #[test]
    fn clean_custom_cache_dir_removes_only_tryke_cache_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache_dir = dir.path().join("custom-cache");
        fs::create_dir_all(&cache_dir).expect("create custom cache dir");
        fs::write(cache_dir.join("discovery-v1.bin"), b"cache").expect("write cache");
        fs::write(cache_dir.join("discovery-v1.tmp"), b"tmp").expect("write tmp cache");
        fs::write(cache_dir.join("keep-me.txt"), b"user data").expect("write user data");
        fs::write(cache_dir.join(".gitignore"), b"custom\n").expect("write user gitignore");

        let report = clean_project_cache(dir.path(), Some(&cache_dir)).expect("clean custom cache");

        assert_eq!(report.cache_dir, cache_dir);
        assert_eq!(report.removed_entries, 2);
        assert!(!report.cache_dir.join("discovery-v1.bin").exists());
        assert!(!report.cache_dir.join("discovery-v1.tmp").exists());
        assert_eq!(
            fs::read_to_string(report.cache_dir.join("keep-me.txt")).expect("read user data"),
            "user data"
        );
        assert_eq!(
            fs::read_to_string(report.cache_dir.join(".gitignore")).expect("read gitignore"),
            "custom\n"
        );
    }

    #[test]
    fn roundtrip_with_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cache.json");
        let mut cache = DiskCache::load(path.clone());
        let key = FileKey {
            mtime_nanos: 12345,
            size: 67,
        };
        let data = DiscoveredFile::default();
        cache.insert(PathBuf::from("foo.py"), key, data.clone());
        cache.save().expect("save");

        let reloaded = DiskCache::load(path);
        assert_eq!(reloaded.entries.len(), 1);
        let got = reloaded
            .get(Path::new("foo.py"), &key)
            .expect("present with matching key");
        assert_eq!(*got, data);
    }

    #[test]
    fn roundtrip_with_populated_test_item() {
        // Regression for the v1 encoding: `TestItem` uses
        // `skip_serializing_if` on nine optional fields. `to_vec`
        // (structs-as-arrays) emitted a variable-length tuple that
        // couldn't be deserialised back, so the whole cache loaded
        // empty. `to_vec_named` encodes as a map and round-trips.
        use tryke_types::{ParsedFile, TestItem};
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cache.bin");
        let mut cache = DiskCache::load(path.clone());
        let key = FileKey {
            mtime_nanos: 12345,
            size: 67,
        };
        let data = DiscoveredFile {
            parsed: ParsedFile {
                tests: vec![TestItem {
                    name: "test_foo".into(),
                    module_path: "tests.test_foo".into(),
                    file_path: Some(PathBuf::from("tests/test_foo.py")),
                    line_number: Some(9),
                    ..TestItem::default()
                }],
                ..ParsedFile::default()
            },
            ..DiscoveredFile::default()
        };
        cache.insert(PathBuf::from("foo.py"), key, data.clone());
        cache.save().expect("save");

        let reloaded = DiskCache::load(path);
        assert_eq!(reloaded.entries.len(), 1);
        let got = reloaded
            .get(Path::new("foo.py"), &key)
            .expect("present with matching key");
        assert_eq!(*got, data);
    }

    #[test]
    fn mismatched_key_misses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cache.json");
        let mut cache = DiskCache::load(path);
        let key = FileKey {
            mtime_nanos: 1,
            size: 1,
        };
        cache.insert(PathBuf::from("foo.py"), key, DiscoveredFile::default());
        let wrong_key = FileKey {
            mtime_nanos: 2,
            size: 1,
        };
        assert!(cache.get(Path::new("foo.py"), &wrong_key).is_none());
    }

    #[test]
    fn corrupted_file_loads_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cache.json");
        fs::write(&path, b"not json").expect("write");
        let cache = DiskCache::load(path);
        assert_eq!(cache.entries.len(), 0);
    }

    #[test]
    fn retain_drops_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cache.json");
        let mut cache = DiskCache::load(path);
        cache.insert(
            PathBuf::from("a.py"),
            FileKey {
                mtime_nanos: 1,
                size: 1,
            },
            DiscoveredFile::default(),
        );
        cache.insert(
            PathBuf::from("b.py"),
            FileKey {
                mtime_nanos: 1,
                size: 1,
            },
            DiscoveredFile::default(),
        );
        let mut keep = std::collections::HashSet::new();
        keep.insert(PathBuf::from("a.py"));
        cache.retain(&keep);
        assert!(
            cache
                .get(
                    Path::new("a.py"),
                    &FileKey {
                        mtime_nanos: 1,
                        size: 1
                    }
                )
                .is_some()
        );
        assert!(
            cache
                .get(
                    Path::new("b.py"),
                    &FileKey {
                        mtime_nanos: 1,
                        size: 1
                    }
                )
                .is_none()
        );
    }
}
