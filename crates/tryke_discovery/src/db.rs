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
    pub path: PathBuf,
}

#[salsa::tracked]
pub fn parse_tests(db: &dyn Db, file: SourceFile) -> ParsedFile {
    crate::parse_tests_from_source(file.root(db), file.path(db), file.text(db))
}

#[salsa::db]
#[derive(Default)]
pub struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {}
