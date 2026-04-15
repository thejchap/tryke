use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::Result;
use tryke_discovery::Discoverer;
use tryke_types::HookItem;

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

/// Print the fixture (`@fixture` + `Depends()`) dependency graph.
///
/// For each discovered hook, prints its qualified name, the hooks it
/// depends on (its `Depends(...)` parameters), and the hooks that depend
/// on it — mirroring the shape of [`run_graph`] for imports. Unresolved
/// dependency names (references to hooks that don't exist in any
/// discovered module) are printed with a `?` suffix so users can spot
/// typos or missing fixtures without reading through test output.
pub fn run_fixture_graph(root: Option<&Path>, excludes: &[String]) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root_path = root.unwrap_or(&cwd);
    let mut discoverer = Discoverer::new_with_excludes(root_path, excludes);
    discoverer.rediscover();

    let hooks = discoverer.hooks();
    if hooks.is_empty() {
        println!("No fixtures discovered.");
        return Ok(());
    }

    // Index hooks by their short (function) name so we can resolve
    // `Depends(foo)` references. A name may resolve to multiple hooks
    // across different modules — we collect all of them, and a reference
    // is "resolved" if at least one match exists.
    let mut by_short_name: HashMap<&str, Vec<&HookItem>> = HashMap::new();
    for hook in &hooks {
        by_short_name
            .entry(hook.name.as_str())
            .or_default()
            .push(hook);
    }

    // Build qualified names and reverse-dependency map. Using BTreeMap
    // for deterministic output ordering.
    let qualified = |h: &HookItem| -> String {
        if h.groups.is_empty() {
            format!("{}::{}", h.module_path, h.name)
        } else {
            format!("{}::{}::{}", h.module_path, h.groups.join("::"), h.name)
        }
    };

    let mut entries: BTreeMap<String, (&HookItem, Vec<String>, Vec<String>)> = BTreeMap::new();
    for hook in &hooks {
        entries
            .entry(qualified(hook))
            .or_insert_with(|| (hook, Vec::new(), Vec::new()));
    }

    // Populate forward deps (from depends_on) and reverse deps.
    for hook in &hooks {
        let q = qualified(hook);
        for dep_name in &hook.depends_on {
            let resolved = by_short_name
                .get(dep_name.as_str())
                .filter(|v| !v.is_empty());
            if let Some(matches) = resolved {
                for target in matches {
                    let target_q = qualified(target);
                    if let Some(entry) = entries.get_mut(&q) {
                        entry.1.push(target_q.clone());
                    }
                    if let Some(target_entry) = entries.get_mut(&target_q) {
                        target_entry.2.push(q.clone());
                    }
                }
            } else if let Some(entry) = entries.get_mut(&q) {
                entry.1.push(format!("{dep_name} (?)"));
            }
        }
    }

    for (q, (hook, deps, rdeps)) in &entries {
        let per = match hook.per {
            tryke_types::FixturePer::Test => "per=test",
            tryke_types::FixturePer::Scope => "per=scope",
        };
        println!("{q}  [{per}]");
        if deps.is_empty() {
            println!("  depends on:  (none)");
        } else {
            println!("  depends on:  {}", deps.join(", "));
        }
        if rdeps.is_empty() {
            println!("  used by:     (none)");
        } else {
            println!("  used by:     {}", rdeps.join(", "));
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

    #[test]
    fn run_fixture_graph_prints_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_fixtures.py"),
            "from tryke import fixture, Depends, test\n\
             @fixture\n\
             def db():\n    yield 1\n\
             @fixture\n\
             def session(conn=Depends(db)):\n    yield conn\n\
             @test\n\
             def test_it(s=Depends(session)):\n    pass\n",
        )
        .expect("write");
        assert!(run_fixture_graph(Some(dir.path()), &[]).is_ok());
    }

    #[test]
    fn run_fixture_graph_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "").expect("write pyproject.toml");
        std::fs::write(
            dir.path().join("test_empty.py"),
            "@test\ndef test_it(): pass\n",
        )
        .expect("write");
        assert!(run_fixture_graph(Some(dir.path()), &[]).is_ok());
    }
}
