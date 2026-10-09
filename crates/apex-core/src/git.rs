//! Git integration implemented by shelling out to the system `git` binary.
//!
//! Shelling out keeps the build free of a native libgit2 dependency, which is
//! important on Windows/GNU toolchains, and matches how agents actually invoke
//! Git. All calls are bounded by [`process::run`].

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{ApexError, Result};
use crate::process::{self, CommandOutput, RunOptions};

fn opts(dir: &Path) -> RunOptions {
    RunOptions::in_dir(dir)
}

async fn git(dir: &Path, args: &[&str]) -> Result<CommandOutput> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let out = process::run("git", &args, &opts(dir)).await?;
    Ok(out)
}

/// A single parsed entry from `git status --porcelain`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusEntry {
    /// Two-character status code (index + worktree).
    pub code: String,
    /// Repository-relative path.
    pub path: String,
}

impl StatusEntry {
    /// Whether the entry represents an untracked file.
    pub fn is_untracked(&self) -> bool {
        self.code.starts_with("??")
    }
}

/// Returns true when `git` is invocable.
pub async fn is_git_available() -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    match git(&cwd, &["--version"]).await {
        Ok(out) => out.success,
        Err(_) => false,
    }
}

/// Returns true when `dir` is inside a Git work tree.
pub async fn is_repo(dir: &Path) -> Result<bool> {
    let out = git(dir, &["rev-parse", "--is-inside-work-tree"]).await?;
    Ok(out.success && out.stdout.trim() == "true")
}

/// Current branch name, or `HEAD` in detached state.
pub async fn current_branch(dir: &Path) -> Result<String> {
    let out = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    if !out.success {
        return Err(ApexError::Git(out.stderr.trim().to_string()));
    }
    Ok(out.stdout.trim().to_string())
}

/// The current HEAD commit hash, if any.
pub async fn head_commit(dir: &Path) -> Result<Option<String>> {
    let out = git(dir, &["rev-parse", "HEAD"]).await?;
    if out.success {
        Ok(Some(out.stdout.trim().to_string()))
    } else {
        Ok(None)
    }
}

/// Parse `git status --porcelain=v1` output.
pub async fn status(dir: &Path) -> Result<Vec<StatusEntry>> {
    let out = git(dir, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    if !out.success {
        return Err(ApexError::Git(out.stderr.trim().to_string()));
    }
    Ok(parse_status(&out.stdout))
}

fn parse_status(text: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.len() < 3 {
            continue;
        }
        let code = &line[..2];
        let mut path = line[3..].to_string();
        // Renames are shown as "old -> new"; keep the destination.
        if let Some((_, dest)) = path.rsplit_once(" -> ") {
            path = dest.to_string();
        }
        entries.push(StatusEntry {
            code: code.to_string(),
            path,
        });
    }
    entries
}

/// Return the unstaged (or staged) diff as a unified patch.
pub async fn diff(dir: &Path, staged: bool) -> Result<String> {
    let mut args = vec!["diff", "--no-color"];
    if staged {
        args.push("--cached");
    }
    let out = git(dir, &args).await?;
    if !out.success {
        return Err(ApexError::Git(out.stderr.trim().to_string()));
    }
    Ok(out.stdout)
}

/// Return a short diffstat summary.
pub async fn diff_stat(dir: &Path) -> Result<String> {
    let out = git(dir, &["diff", "--stat", "--no-color"]).await?;
    if !out.success {
        return Err(ApexError::Git(out.stderr.trim().to_string()));
    }
    Ok(out.stdout)
}

/// Return the combined diff of all changes (staged + unstaged + untracked)
/// as text suitable for review. Untracked files are listed but not expanded.
pub async fn full_diff(dir: &Path) -> Result<String> {
    let mut text = diff(dir, false).await?;
    let staged = diff(dir, true).await?;
    if !staged.is_empty() {
        text.push_str("\n# --- staged changes ---\n");
        text.push_str(&staged);
    }
    let status = status(dir).await?;
    let untracked: Vec<&str> = status
        .iter()
        .filter(|e| e.is_untracked())
        .map(|e| e.path.as_str())
        .collect();
    if !untracked.is_empty() {
        text.push_str("\n# --- untracked files ---\n");
        for path in untracked {
            text.push_str(&format!("{path}\n"));
        }
    }
    Ok(text)
}

/// Stage every change in the work tree.
pub async fn add_all(dir: &Path) -> Result<()> {
    let out = git(dir, &["add", "-A"]).await?;
    if !out.success {
        return Err(ApexError::Git(out.stderr.trim().to_string()));
    }
    Ok(())
}

/// Create a commit with the given message and return its hash.
pub async fn commit(dir: &Path, message: &str) -> Result<String> {
    let out = git(dir, &["commit", "-m", message]).await?;
    if !out.success {
        return Err(ApexError::Git(format!(
            "commit failed: {}{}",
            out.stdout.trim(),
            out.stderr.trim()
        )));
    }
    head_commit(dir)
        .await?
        .ok_or_else(|| ApexError::Git("commit succeeded but HEAD is missing".into()))
}

/// Initialise a new Git repository in `dir`.
pub async fn init(dir: &Path) -> Result<()> {
    let out = git(dir, &["init"]).await?;
    if !out.success {
        return Err(ApexError::Git(out.stderr.trim().to_string()));
    }
    Ok(())
}

/// Configure a local commit identity if none is set, so test repositories can
/// commit without touching the user's global Git configuration.
pub async fn ensure_identity(dir: &Path) -> Result<()> {
    let _ = git(dir, &["config", "user.email", "apex@localhost"]).await?;
    let _ = git(dir, &["config", "user.name", "APEX"]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_lines() {
        let text = "M  src/main.rs\n?? new.txt\nR  old.rs -> new.rs\n";
        let entries = parse_status(text);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].code, "M ");
        assert_eq!(entries[0].path, "src/main.rs");
        assert!(entries[1].is_untracked());
        assert_eq!(entries[2].path, "new.rs");
    }
}
