use std::path::PathBuf;

use tryke_types::ParsedFile;

#[salsa::db]
pub trait Db: salsa::Database {}

#[salsa::input]
pub struct SourceFile {
    #[returns(ref)]
    pub text: String,
    #[returns(ref)]
    pub root: PathBuf,
    #[returns(ref)]
    pub src_roots: Vec<PathBuf>,
    #[returns(ref)]
    pub path: PathBuf,
}

/// Everything derivable from a single parse of a Python source file:
/// the `ParsedFile` (tests, hooks, guard-else lines, errors), the
/// candidate import paths this file references, and the dynamic-import
/// flag. Produced in one AST walk so callers never parse the file
/// twice. `import_candidates` holds first-wins alternatives that the
/// discoverer resolves against the project's enumerated file set,
/// avoiding per-import `stat()` syscalls.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredFile {
    pub parsed: ParsedFile,
    pub import_candidates: Vec<Vec<PathBuf>>,
    pub dynamic_imports: bool,
}

#[salsa::tracked]
pub fn discover_file(db: &dyn Db, file: SourceFile) -> DiscoveredFile {
    crate::discover_file_from_source(
        file.root(db),
        file.src_roots(db),
        file.path(db),
        file.text(db),
    )
}

#[salsa::db]
#[derive(Default, Clone)]
pub struct Database {
    storage: salsa::Storage<Self>,
}

impl Database {
    /// Produce a `Sync` snapshot handle that can be sent across threads and
    /// used to build per-thread `Database` instances via [`from_handle`].
    /// Cloning the handle is cheap (Arc bump); each `from_handle` yields a
    /// fresh per-thread salsa local, sharing the same memo tables.
    #[must_use]
    pub fn storage_handle(&self) -> salsa::StorageHandle<Self> {
        self.storage.clone().into_zalsa_handle()
    }

    #[must_use]
    pub fn from_handle(handle: salsa::StorageHandle<Self>) -> Self {
        Self {
            storage: handle.into_storage(),
        }
    }
}

impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {}
