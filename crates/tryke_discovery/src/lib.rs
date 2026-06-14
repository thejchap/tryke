mod source;

#[cfg(feature = "filesystem")]
mod filesystem;

pub use source::{
    discover_file_from_source, parse_tests_from_source, resolve_import_candidate_groups,
};

#[cfg(feature = "filesystem")]
pub use filesystem::{
    CleanCacheReport, Discoverer, build_change_set_ignore, clean_project_cache,
    configured_excludes, discover, discover_from, discover_from_with_excludes,
    discover_from_with_options, resolve_src_roots,
};
#[cfg(not(feature = "filesystem"))]
pub use source::resolve_src_roots;

#[cfg(feature = "filesystem")]
pub(crate) use source::path_to_module;
