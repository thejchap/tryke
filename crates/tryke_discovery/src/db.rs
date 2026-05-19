use std::path::PathBuf;

pub use tryke_types::DiscoveredFile;

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
