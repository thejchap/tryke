pub mod pool;
pub mod protocol;
pub mod worker;

pub use pool::{WorkerPool, path_to_module};
pub use worker::WorkerProcess;

use std::path::Path;

#[must_use]
pub fn resolve_python(root: &Path) -> String {
    let venv = root.join(".venv/bin/python3");
    if venv.exists() {
        venv.to_string_lossy().into_owned()
    } else {
        "python3".to_owned()
    }
}
