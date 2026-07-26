//! Shared dev helpers used across tryke crates' test modules.
//!
//! Lives outside the production crates so the same logic isn't copy-pasted
//! into five different test modules and quietly drift apart (the venv
//! layout and Windows-vs-Unix fallback are easy to get wrong in only one
//! place).

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

use tryke_config::Project;

/// A temporary on-disk Tryke project owned by a test.
///
/// The project always starts with an empty `pyproject.toml`, so project-root
/// discovery is deterministic. Dropping this value removes the entire
/// temporary directory.
pub struct TestProject {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl TestProject {
    /// Create an empty temporary Tryke project.
    ///
    /// # Errors
    ///
    /// Returns an error if the temporary directory cannot be created,
    /// canonicalized, or initialized with `pyproject.toml`.
    pub fn new() -> io::Result<Self> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        let project = Self {
            _directory: directory,
            root,
        };
        project.write("pyproject.toml", [])?;
        Ok(project)
    }

    /// Create a temporary Tryke project populated with `files`.
    ///
    /// A supplied `pyproject.toml` replaces the empty default created by
    /// [`Self::new`].
    ///
    /// # Errors
    ///
    /// Returns an error if the project or any supplied file cannot be created.
    pub fn with_files<I, P, C>(files: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = (P, C)>,
        P: AsRef<Path>,
        C: AsRef<[u8]>,
    {
        let project = Self::new()?;
        for (path, contents) in files {
            project.write(path, contents)?;
        }
        Ok(project)
    }

    /// Return the canonical project root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Write a project-relative file, creating its parent directories.
    ///
    /// # Errors
    ///
    /// Returns an error if `relative` is empty, absolute, or contains a parent
    /// traversal, or if the directory or file cannot be written.
    pub fn write<P, C>(&self, relative: P, contents: C) -> io::Result<PathBuf>
    where
        P: AsRef<Path>,
        C: AsRef<[u8]>,
    {
        let path = self.resolve(relative.as_ref())?;
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "file path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        fs::write(&path, contents)?;
        Ok(path)
    }

    /// Remove a project-relative file.
    ///
    /// # Errors
    ///
    /// Returns an error if `relative` is empty, absolute, or contains a parent
    /// traversal, or if the file cannot be removed.
    pub fn remove_file<P>(&self, relative: P) -> io::Result<()>
    where
        P: AsRef<Path>,
    {
        fs::remove_file(self.resolve(relative.as_ref())?)
    }

    /// Resolve the current project files and configuration.
    #[must_use]
    pub fn project(&self) -> Project {
        Project::discover(self.root())
    }

    fn resolve(&self, relative: &Path) -> io::Result<PathBuf> {
        let mut path = self.root.clone();
        let mut has_normal_component = false;

        for component in relative.components() {
            match component {
                Component::Normal(component) => {
                    path.push(component);
                    has_normal_component = true;
                }
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "test project paths must stay relative to the project root: {}",
                            relative.display()
                        ),
                    ));
                }
            }
        }

        if !has_normal_component {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "test project path must name a file",
            ));
        }

        Ok(path)
    }
}

/// Path to the workspace root, derived from this crate's manifest dir.
///
/// Anchoring on this crate's `CARGO_MANIFEST_DIR` (rather than the
/// caller's) makes the helper callable from any test module in the
/// workspace without each crate computing its own relative offset.
#[must_use]
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Returns a Python interpreter suitable for spawning workers in tests.
///
/// Uses the same environment discovery as production.
#[must_use]
pub fn python_bin() -> String {
    Project::discover(&workspace_root()).python().to_owned()
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn creates_default_project() -> io::Result<()> {
        let project = TestProject::new()?;

        assert!(project.root().join("pyproject.toml").is_file());
        assert_eq!(project.project().root(), project.root());
        Ok(())
    }

    #[test]
    fn writes_nested_files() -> io::Result<()> {
        let project = TestProject::new()?;
        let path = project.write("src/pkg/test_api.py", b"VALUE = 1\n")?;

        assert_eq!(path, project.root().join("src/pkg/test_api.py"));
        assert_eq!(fs::read(path)?, b"VALUE = 1\n");
        Ok(())
    }

    #[test]
    fn creates_project_with_files_and_configuration() -> io::Result<()> {
        let project = TestProject::with_files([
            (
                "pyproject.toml",
                "[tool.tryke]\nexclude = [\"generated\"]\n",
            ),
            ("tests/test_api.py", "@test\ndef test_api(): pass\n"),
        ])?;

        assert!(project.root().join("tests/test_api.py").is_file());
        assert_eq!(
            project.project().discovery().exclude,
            vec!["generated".to_string()]
        );
        Ok(())
    }

    #[test]
    fn removes_files() -> io::Result<()> {
        let project = TestProject::with_files([("test_api.py", "")])?;

        project.remove_file("test_api.py")?;

        assert!(!project.root().join("test_api.py").exists());
        Ok(())
    }

    #[test]
    fn rejects_paths_outside_project() -> io::Result<()> {
        let project = TestProject::new()?;

        for path in [Path::new(""), Path::new("../outside.py"), project.root()] {
            let error = project
                .write(path, "")
                .expect_err("path outside the project must be rejected");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
        Ok(())
    }

    #[test]
    fn drop_removes_project() -> io::Result<()> {
        let root = {
            let project = TestProject::new()?;
            project.root().to_path_buf()
        };

        assert!(!root.exists());
        Ok(())
    }
}
