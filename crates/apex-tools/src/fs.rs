//! Filesystem tools: read, write, edit and list.

use std::path::Path;

use apex_core::config::RiskClass;
use apex_core::error::{ApexError, Result};
use apex_core::paths::resolve_within;
use apex_protocol::ToolSpec;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::types::{truncate_content, Tool, ToolContext, ToolResult};

fn path_arg(args: &Value) -> Result<String> {
    args.get("path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ApexError::tool("fs", "missing required string argument 'path'"))
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn guard_write_target(ctx: &ToolContext, path: &Path) -> Result<()> {
    if path.components().any(|c| c.as_os_str() == ".git") {
        return Err(ApexError::Permission(
            "refusing to write inside the .git directory".into(),
        ));
    }
    let _ = ctx;
    Ok(())
}

/// Read a file within the workspace.
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Read a UTF-8 text file from the workspace. Returns its contents.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path relative to the workspace root."},
                    "offset": {"type": "integer", "description": "1-based line to start from."},
                    "limit": {"type": "integer", "description": "Maximum number of lines to return."}
                },
                "required": ["path"]
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::ReadOnly
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let raw = path_arg(&args)?;
        let path = resolve_within(&ctx.workspace_root, &raw)?;
        if path.is_dir() {
            return Err(ApexError::tool(
                self.name(),
                format!("{raw} is a directory"),
            ));
        }
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ApexError::tool(self.name(), format!("could not read {raw}: {e}")))?;

        let lines: Vec<&str> = text.lines().collect();
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .max(1) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;
        let start = (offset - 1).min(lines.len());
        let end = (start + limit).min(lines.len());

        let mut numbered = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            numbered.push_str(&format!("{:>5}\t{}\n", start + i + 1, line));
        }
        if end < lines.len() {
            numbered.push_str(&format!("… {} more lines\n", lines.len() - end));
        }

        let (content, truncated) = truncate_content(&numbered, ctx.max_content_bytes());
        let rel = relative_display(&ctx.workspace_root, &path);
        Ok(ToolResult {
            ok: true,
            summary: format!("read {rel} ({} lines)", lines.len()),
            content,
            data: json!({"path": rel, "lines": lines.len()}),
            truncated,
        })
    }
}

/// Write (create or overwrite) a file within the workspace.
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Create or overwrite a text file in the workspace.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path relative to the workspace root."},
                    "content": {"type": "string", "description": "Full file contents to write."}
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::Mutating
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let raw = path_arg(&args)?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ApexError::tool(self.name(), "missing required string argument 'content'")
            })?;
        let path = resolve_within(&ctx.workspace_root, &raw)?;
        guard_write_target(ctx, &path)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        let rel = relative_display(&ctx.workspace_root, &path);
        Ok(ToolResult::success_with_data(
            format!("wrote {rel} ({} bytes)", content.len()),
            format!("Wrote {rel} ({} bytes).", content.len()),
            json!({"path": rel, "bytes": content.len()}),
        ))
    }
}

/// Replace an exact substring in a file within the workspace.
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Replace an exact text snippet in a workspace file. The old text must match exactly.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_string": {"type": "string", "description": "Exact text to replace."},
                    "new_string": {"type": "string", "description": "Replacement text."},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence."}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::Mutating
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let raw = path_arg(&args)?;
        let old = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApexError::tool(self.name(), "missing 'old_string'"))?;
        let new = args
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApexError::tool(self.name(), "missing 'new_string'"))?;
        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let path = resolve_within(&ctx.workspace_root, &raw)?;
        guard_write_target(ctx, &path)?;
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ApexError::tool(self.name(), format!("could not read {raw}: {e}")))?;

        let count = text.matches(old).count();
        if count == 0 {
            return Err(ApexError::tool(
                self.name(),
                format!("old_string not found in {raw}"),
            ));
        }
        if count > 1 && !replace_all {
            return Err(ApexError::tool(
                self.name(),
                format!(
                    "old_string occurs {count} times in {raw}; pass replace_all or add context"
                ),
            ));
        }
        let updated = if replace_all {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        };
        tokio::fs::write(&path, &updated).await?;
        let rel = relative_display(&ctx.workspace_root, &path);
        Ok(ToolResult::success_with_data(
            format!("edited {rel} ({count} replacement(s))"),
            format!("Updated {rel}: replaced {count} occurrence(s)."),
            json!({"path": rel, "replacements": count}),
        ))
    }
}

/// List a directory within the workspace.
pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "List entries in a workspace directory.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path (defaults to the workspace root)."}
                }
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::ReadOnly
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let path = resolve_within(&ctx.workspace_root, raw)?;
        let mut entries = Vec::new();
        let mut reader = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| ApexError::tool(self.name(), format!("could not list {raw}: {e}")))?;
        while let Some(entry) = reader.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push(if is_dir { format!("{name}/") } else { name });
        }
        entries.sort();
        let rel = relative_display(&ctx.workspace_root, &path);
        let content = entries.join("\n");
        Ok(ToolResult::success_with_data(
            format!("listed {rel} ({} entries)", entries.len()),
            content,
            json!({"path": rel, "entries": entries}),
        ))
    }
}
