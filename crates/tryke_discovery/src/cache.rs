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

use crate::db::DiscoveredFile;

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

impl DiskCache {
    /// Load a cache from `path`. Returns an empty cache if the file is
    /// missing, corrupted, or has a mismatched `CACHE_VERSION`.
    pub fn load(path: PathBuf) -> Self {
        let gitignore = path
            .parent()
            .and_then(Path::parent)
            .map(|dir| GitignoreConfig {
                dir: dir.to_path_buf(),
                contents: "# created by tryke\n*\n",
            });
        Self::load_with_gitignore(path, gitignore)
    }

    /// Load the standard discovery cache file inside `cache_dir`.
    ///
    /// `cache_dir` is also the directory that receives the best-effort
    /// `.gitignore`, matching user intent for a custom cache location. Unlike
    /// the default `.tryke` state directory, this writes narrow patterns only:
    /// a user-provided cache directory may be an existing project directory.
    pub fn load_in_dir(cache_dir: PathBuf) -> Self {
        let path = cache_dir.join("discovery-v1.bin");
        Self::load_with_gitignore(
            path,
            Some(GitignoreConfig {
                dir: cache_dir,
                contents: "# created by tryke\n/discovery-v1.bin\n/discovery-v1.tmp\n",
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
        let path = dir.path().join("cache.json");
        let cache = DiskCache::load(path.clone());
        assert_eq!(cache.entries.len(), 0);
        cache.save().expect("save");
        let reloaded = DiskCache::load(path);
        assert_eq!(reloaded.entries.len(), 0);
    }

    #[test]
    fn default_cache_writes_broad_gitignore_in_state_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir
            .path()
            .join(".tryke")
            .join("cache")
            .join("discovery-v1.bin");
        let cache = DiskCache::load(path);

        cache.save().expect("save");

        let gitignore =
            fs::read_to_string(dir.path().join(".tryke/.gitignore")).expect("read gitignore");
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
            "# created by tryke\n/discovery-v1.bin\n/discovery-v1.tmp\n"
        );
        assert!(!gitignore.lines().any(|line| line.trim() == "*"));
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
