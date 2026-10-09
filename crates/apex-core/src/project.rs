//! Project discovery.

use std::path::{Path, PathBuf};

use crate::error::{ApexError, Result};

/// A discovered project rooted at a directory.
#[derive(Debug, Clone)]
pub struct Project {
    /// Absolute path to the project root.
    pub root: PathBuf,
    /// Human-readable project name.
    pub name: String,
    /// Whether the root contains a Git repository.
    pub is_git: bool,
}

impl Project {
    /// Discover a project starting from `start`, walking up to the nearest
    /// directory containing a `.git` entry. Falls back to `start` itself.
    pub fn discover(start: impl AsRef<Path>) -> Result<Project> {
        let start = start.as_ref();
        let start = if start.is_absolute() {
            start.to_path_buf()
        } else {
            std::env::current_dir()?.join(start)
        };

        let mut candidate = canonicalize_lossy(&start);
        let mut git_root: Option<PathBuf> = None;
        loop {
            if candidate.join(".git").exists() {
                git_root = Some(candidate.clone());
                break;
            }
            match candidate.parent() {
                Some(parent) if parent != candidate => candidate = parent.to_path_buf(),
                _ => break,
            }
        }

        let root = git_root
            .clone()
            .unwrap_or_else(|| canonicalize_lossy(&start));
        let name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "project".into());

        Ok(Project {
            is_git: git_root.is_some(),
            root,
            name,
        })
    }

    /// Metadata directory for APEX within this project.
    pub fn apex_dir(&self) -> PathBuf {
        self.root.join(".apex")
    }

    /// Ensure the project `.apex` directory exists.
    pub fn ensure_apex_dir(&self) -> Result<PathBuf> {
        let dir = self.apex_dir();
        crate::paths::ensure_dir(&dir)?;
        Ok(dir)
    }

    /// Verify that this project is a Git repository.
    pub fn require_git(&self) -> Result<()> {
        if self.is_git {
            Ok(())
        } else {
            Err(ApexError::Project(format!(
                "{} is not a Git repository",
                self.root.display()
            )))
        }
    }
}

fn canonicalize_lossy(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
