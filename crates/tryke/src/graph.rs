use std::path::Path;

use anyhow::Result;
use tryke_discovery::Discoverer;

use crate::git::resolve_changed_files;

pub fn run_graph(
    root: Option<&Path>,
    excludes: &[String],
    connected_only: bool,
    changed: bool,
    base_branch: Option<&str>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root_path = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new_with_excludes(root_path, excludes);
    discoverer.rediscover();

    let changed_files = if changed {
        match resolve_changed_files(root_path, base_branch) {
            Some(paths) if !paths.is_empty() => Some(paths),
            Some(_) => {
                println!("No git-visible changed files found.");
                return Ok(());
            }
            None => {
                println!("Git unavailable or failed; cannot compute changed graph.");
                return Ok(());
            }
        }
    } else {
        None
    };

    let affected = changed_files
        .as_ref()
        .map(|paths| discoverer.affected_files(paths))
        .unwrap_or_default();
    let changed_set = changed_files
        .as_ref()
        .map(|paths| {
            paths
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let summary = discoverer.import_graph_summary();
    for entry in &summary {
        let full_path = root_path.join(&entry.file);
        if changed && !affected.contains(&full_path) {
            continue;
        }
        if connected_only && entry.imports.is_empty() && entry.imported_by.is_empty() {
            continue;
        }
        let label = if changed {
            if changed_set.contains(&full_path) {
                " [changed]"
            } else {
                " [affected]"
            }
        } else {
            ""
        };
        println!("{}{}", entry.file.display(), label);
        if entry.imports.is_empty() {
            println!("  imports:     (none)");
        } else {
            let names: Vec<String> = entry
                .imports
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            println!("  imports:     {}", names.join(", "));
        }
        if entry.imported_by.is_empty() {
            println!("  imported by: (none)");
        } else {
            let names: Vec<String> = entry
                .imported_by
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            println!("  imported by: {}", names.join(", "));
        }
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_graph_prints_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(dir.path().join("utils.py"), "def helper(): pass\n").expect("write");
        std::fs::write(
            dir.path().join("test_foo.py"),
            "from utils import helper\n@test\ndef test_foo(): pass\n",
        )
        .expect("write");
        assert!(run_graph(Some(dir.path()), &[], false, false, None).is_ok());
    }

    #[test]
    fn run_graph_connected_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(dir.path().join("utils.py"), "def helper(): pass\n").expect("write");
        std::fs::write(
            dir.path().join("test_foo.py"),
            "from utils import helper\n@test\ndef test_foo(): pass\n",
        )
        .expect("write");
        std::fs::write(
            dir.path().join("test_isolated.py"),
            "@test\ndef test_isolated(): pass\n",
        )
        .expect("write");
        assert!(run_graph(Some(dir.path()), &[], true, false, None).is_ok());
    }
}
