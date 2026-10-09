//! Filesystem locations used by APEX.

use std::path::{Path, PathBuf};

use crate::error::{ApexError, Result};

/// Environment variable that overrides the APEX home directory.
pub const APEX_HOME_ENV: &str = "APEX_HOME";

/// Returns the APEX home directory, creating it if necessary.
///
/// Defaults to `~/.apex` on all platforms unless `APEX_HOME` is set.
pub fn apex_home() -> Result<PathBuf> {
    let home = match std::env::var_os(APEX_HOME_ENV) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => dirs::home_dir()
            .ok_or_else(|| ApexError::config("could not determine the user home directory"))?
            .join(".apex"),
    };
    ensure_dir(&home)?;
    Ok(home)
}

/// Directory holding persistent runtime state (database, sockets, logs).
pub fn state_dir() -> Result<PathBuf> {
    let dir = apex_home()?.join("state");
    ensure_dir(&dir)?;
    Ok(dir)
}

/// Directory holding user-level configuration.
pub fn config_dir() -> Result<PathBuf> {
    let dir = apex_home()?.join("config");
    ensure_dir(&dir)?;
    Ok(dir)
}

/// Directory holding installed agents.
pub fn agents_dir() -> Result<PathBuf> {
    let dir = apex_home()?.join("agents");
    ensure_dir(&dir)?;
    Ok(dir)
}

/// Directory holding installed skills.
pub fn skills_dir() -> Result<PathBuf> {
    let dir = apex_home()?.join("skills");
    ensure_dir(&dir)?;
    Ok(dir)
}

/// Path to the SQLite database for local state.
pub fn database_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("apex.db"))
}

/// Create a directory (and parents) if it does not already exist.
pub fn ensure_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

/// Resolve `candidate` (which may be relative or absolute) and guarantee that
/// the result stays inside `root`. Prevents path traversal via `..`,
/// absolute paths, or symlink escapes.
///
/// The path does not need to exist. If it does exist, it is canonicalized so
/// that symlinks are resolved before the boundary check.
pub fn resolve_within(root: &Path, candidate: impl AsRef<Path>) -> Result<PathBuf> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let candidate = candidate.as_ref();

    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };

    // Lexically normalise to collapse `.` and `..` without touching the FS.
    let normalised = lexical_normalize(&joined);

    // Verify the lexical prefix first.
    if !normalised.starts_with(&root) {
        return Err(ApexError::PathEscape {
            path: normalised,
            root,
        });
    }

    // If it exists, resolve symlinks and re-check.
    if normalised.exists() {
        let real = std::fs::canonicalize(&normalised)?;
        if !real.starts_with(&root) {
            return Err(ApexError::PathEscape { path: real, root });
        }
        return Ok(real);
    }

    Ok(normalised)
}

/// Lexically normalise a path by collapsing `.` and `..` components.
fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
