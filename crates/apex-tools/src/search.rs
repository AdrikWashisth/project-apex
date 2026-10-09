//! Search tools: glob file discovery and regex content search.

use apex_core::config::RiskClass;
use apex_core::error::{ApexError, Result};
use apex_core::paths::resolve_within;
use apex_protocol::ToolSpec;
use async_trait::async_trait;
use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::types::{truncate_content, Tool, ToolContext, ToolResult};

/// Directories that are never searched.
const IGNORED_DIRS: &[&str] = &[".git", "target", "node_modules", ".apex", "dist", "build"];

fn is_ignored(path: &std::path::Path) -> bool {
    path.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        IGNORED_DIRS.contains(&name.as_ref())
    })
}

/// Find files matching a glob pattern.
pub struct GlobFilesTool;

#[async_trait]
impl Tool for GlobFilesTool {
    fn name(&self) -> &str {
        "glob_files"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Find files by glob pattern (e.g. '**/*.rs') within the workspace.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern, relative to the workspace root."},
                    "max_results": {"type": "integer", "description": "Maximum number of matches (default 200)."}
                },
                "required": ["pattern"]
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::ReadOnly
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApexError::tool(self.name(), "missing 'pattern'"))?;
        let max = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(200) as usize;
        let compiled = glob::Pattern::new(pattern)
            .map_err(|e| ApexError::tool(self.name(), format!("invalid glob: {e}")))?;

        let mut matches = Vec::new();
        for entry in WalkDir::new(&ctx.workspace_root)
            .into_iter()
            .filter_entry(|e| !is_ignored(e.path()))
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = match entry.path().strip_prefix(&ctx.workspace_root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if compiled.matches(&rel_str) || compiled.matches_path(rel) {
                matches.push(rel_str);
                if matches.len() >= max {
                    break;
                }
            }
        }
        matches.sort();
        let content = if matches.is_empty() {
            format!("No files match '{pattern}'.")
        } else {
            matches.join("\n")
        };
        Ok(ToolResult::success_with_data(
            format!("{} file(s) match {}", matches.len(), pattern),
            content,
            json!({"matches": matches}),
        ))
    }
}

/// Search file contents with a regular expression.
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description:
                "Search file contents with a regular expression. Returns file:line matches.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Rust regular expression."},
                    "path": {"type": "string", "description": "Directory or file to search (default workspace root)."},
                    "include": {"type": "string", "description": "Optional glob to restrict filenames, e.g. '*.rs'."},
                    "max_results": {"type": "integer", "description": "Maximum matches (default 200)."}
                },
                "required": ["pattern"]
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::ReadOnly
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApexError::tool(self.name(), "missing 'pattern'"))?;
        let re = regex::Regex::new(pattern)
            .map_err(|e| ApexError::tool(self.name(), format!("invalid regex: {e}")))?;
        let search_root = resolve_within(
            &ctx.workspace_root,
            args.get("path").and_then(|v| v.as_str()).unwrap_or("."),
        )?;
        let include =
            match args.get("include").and_then(|v| v.as_str()) {
                Some(glob_pat) => Some(glob::Pattern::new(glob_pat).map_err(|e| {
                    ApexError::tool(self.name(), format!("invalid include glob: {e}"))
                })?),
                None => None,
            };
        let max = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(200) as usize;

        let files: Vec<std::path::PathBuf> = if search_root.is_file() {
            vec![search_root.clone()]
        } else {
            WalkDir::new(&search_root)
                .into_iter()
                .filter_entry(|e| !is_ignored(e.path()))
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .map(|e| e.path().to_path_buf())
                .collect()
        };

        let mut hits = Vec::new();
        let mut files_scanned = 0usize;
        'outer: for file in files {
            let name = file
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(pat) = &include {
                if !pat.matches(&name) {
                    continue;
                }
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            files_scanned += 1;
            let rel = file
                .strip_prefix(&ctx.workspace_root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(format!("{rel}:{}: {}", i + 1, line.trim_end()));
                    if hits.len() >= max {
                        break 'outer;
                    }
                }
            }
        }

        let content = if hits.is_empty() {
            format!("No matches for '{pattern}'.")
        } else {
            hits.join("\n")
        };
        let (content, truncated) = truncate_content(&content, ctx.max_content_bytes());
        Ok(ToolResult {
            ok: true,
            summary: format!("{} match(es) across {} file(s)", hits.len(), files_scanned),
            content,
            data: json!({"matches": hits.len(), "files_scanned": files_scanned}),
            truncated,
        })
    }
}
