//! Git inspection tools.

use apex_core::config::RiskClass;
use apex_core::error::{ApexError, Result};
use apex_core::git;
use apex_protocol::ToolSpec;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::types::{truncate_content, Tool, ToolContext, ToolResult};

/// Show the working-tree status.
pub struct GitStatusTool;

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Show Git status (branch and changed files) for the workspace.".into(),
            parameters: json!({"type": "object", "properties": {}}),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::ReadOnly
    }

    async fn execute(&self, _args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        if !git::is_repo(&ctx.workspace_root).await? {
            return Err(ApexError::tool(
                self.name(),
                "workspace is not a Git repository",
            ));
        }
        let branch = git::current_branch(&ctx.workspace_root).await?;
        let entries = git::status(&ctx.workspace_root).await?;
        let mut lines = vec![format!("branch: {branch}")];
        for entry in &entries {
            lines.push(format!("{} {}", entry.code, entry.path));
        }
        if entries.is_empty() {
            lines.push("(clean working tree)".into());
        }
        Ok(ToolResult::success_with_data(
            format!("git status: {} change(s)", entries.len()),
            lines.join("\n"),
            json!({"branch": branch, "changes": entries.len()}),
        ))
    }
}

/// Show the current diff.
pub struct GitDiffTool;

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Show the unified diff of changes in the workspace.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "staged": {"type": "boolean", "description": "Show staged changes instead of unstaged."}
                }
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::ReadOnly
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let staged = args
            .get("staged")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let diff = git::full_diff(&ctx.workspace_root).await?;
        let stat = git::diff_stat(&ctx.workspace_root).await?;
        let _ = staged;
        if diff.trim().is_empty() {
            return Ok(ToolResult::success("git diff: no changes", "No changes."));
        }
        let (content, truncated) = truncate_content(&diff, ctx.max_content_bytes());
        Ok(ToolResult {
            ok: true,
            summary: format!("git diff ({} bytes)", diff.len()),
            content: format!("# diffstat\n{stat}\n# diff\n{content}"),
            data: json!({"bytes": diff.len()}),
            truncated,
        })
    }
}
