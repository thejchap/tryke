use std::path::PathBuf;

use log::trace;
use ruff_python_ast::{ModModule, Stmt};
use ruff_python_parser::parse_module;

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

/// Parsed Python source cached as the first incremental layer.
///
/// Equality intentionally ignores raw source text. If the parser produces the
/// same AST body for a new source string, Salsa keeps the old value and
/// backdates dependents, so discovery is not re-run for trivia-only edits.
#[derive(Debug, Clone)]
pub(crate) struct ParsedAst {
    source: String,
    syntax: Option<ModModule>,
}

impl ParsedAst {
    pub(crate) fn parse(source: &str) -> Self {
        let syntax = parse_module(source)
            .ok()
            .map(ruff_python_parser::Parsed::into_syntax);
        Self {
            source: source.to_owned(),
            syntax,
        }
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn syntax(&self) -> Option<&ModModule> {
        self.syntax.as_ref()
    }

    fn body(&self) -> Option<&[Stmt]> {
        self.syntax.as_ref().map(|module| module.body.as_slice())
    }
}

impl PartialEq for ParsedAst {
    fn eq(&self, other: &Self) -> bool {
        self.body() == other.body()
    }
}

#[cfg(test)]
static DISCOVER_FILE_EXECUTIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
static COUNTED_DISCOVER_FILE_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) fn count_discover_file_executions_for(path: PathBuf) {
    *COUNTED_DISCOVER_FILE_PATH.lock().expect("counter mutex") = Some(path);
    DISCOVER_FILE_EXECUTIONS.store(0, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn discover_file_executions() -> usize {
    DISCOVER_FILE_EXECUTIONS.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
fn count_discover_file_execution(path: &std::path::Path) {
    let counted = COUNTED_DISCOVER_FILE_PATH.lock().expect("counter mutex");
    if counted.as_deref() == Some(path) {
        DISCOVER_FILE_EXECUTIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[salsa::tracked(returns(ref))]
pub(crate) fn parse_file(db: &dyn Db, file: SourceFile) -> ParsedAst {
    let path = file.path(db);
    trace!(
        "parsing {}",
        path.strip_prefix(file.root(db)).unwrap_or(path).display()
    );
    ParsedAst::parse(file.text(db))
}

#[salsa::tracked]
pub fn discover_file(db: &dyn Db, file: SourceFile) -> DiscoveredFile {
    #[cfg(test)]
    count_discover_file_execution(file.path(db));

    super::discover_file_from_ast(
        file.root(db),
        file.src_roots(db),
        file.path(db),
        parse_file(db, file),
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
