//! Core tool types: context, results and the `Tool` trait.

use std::path::PathBuf;

use apex_core::config::{PermissionProfile, Permissions, RiskClass};
use apex_core::error::Result;
use apex_protocol::ToolSpec;
use async_trait::async_trait;
use serde_json::Value;

/// Execution context passed to every tool invocation.
#[derive(Clone)]
pub struct ToolContext {
    /// Root of the workspace; all paths are confined to it.
    pub workspace_root: PathBuf,
    /// Active permission policy.
    pub permissions: Permissions,
}

impl ToolContext {
    pub fn new(workspace_root: impl Into<PathBuf>, permissions: Permissions) -> Self {
        ToolContext {
            workspace_root: workspace_root.into(),
            permissions,
        }
    }

    /// Maximum bytes of file/output content returned to the model.
    pub fn max_content_bytes(&self) -> usize {
        200_000
    }

    /// Return a human-readable denial reason if the tool may not run.
    pub fn deny_reason(&self, tool: &str, risk: RiskClass) -> Option<String> {
        let profile = self.permissions.profile;
        match profile {
            PermissionProfile::ReadOnly if risk > RiskClass::ReadOnly => Some(format!(
                "permission profile 'read_only' does not permit '{tool}' (risk: {risk:?})"
            )),
            _ if risk == RiskClass::Executing && !self.permissions.allow_shell => Some(format!(
                "shell execution is disabled; '{tool}' is not permitted"
            )),
            _ => None,
        }
    }

    /// Whether the given risk class requires explicit human approval.
    pub fn needs_approval(&self, risk: RiskClass) -> bool {
        self.permissions.require_approval.contains(&risk)
    }
}

/// The structured result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Whether the tool succeeded.
    pub ok: bool,
    /// One-line summary for event streams.
    pub summary: String,
    /// Model-facing textual content.
    pub content: String,
    /// Structured data for programmatic consumers.
    pub data: Value,
    /// Whether content was truncated to fit limits.
    pub truncated: bool,
}

impl ToolResult {
    pub fn success(summary: impl Into<String>, content: impl Into<String>) -> Self {
        ToolResult {
            ok: true,
            summary: summary.into(),
            content: content.into(),
            data: Value::Null,
            truncated: false,
        }
    }

    pub fn success_with_data(
        summary: impl Into<String>,
        content: impl Into<String>,
        data: Value,
    ) -> Self {
        ToolResult {
            ok: true,
            summary: summary.into(),
            content: content.into(),
            data,
            truncated: false,
        }
    }

    pub fn failure(summary: impl Into<String>, content: impl Into<String>) -> Self {
        ToolResult {
            ok: false,
            summary: summary.into(),
            content: content.into(),
            data: Value::Null,
            truncated: false,
        }
    }
}

/// A tool that an agent may invoke.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name as exposed to the model.
    fn name(&self) -> &str;

    /// Model-facing specification.
    fn spec(&self) -> ToolSpec;

    /// Risk classification.
    fn risk(&self) -> RiskClass;

    /// Execute the tool with parsed JSON arguments.
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult>;
}

/// Truncate text to `max` bytes on a char boundary.
pub fn truncate_content(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (
        format!("{}\n… [truncated {} bytes]", &text[..end], text.len() - end),
        true,
    )
}
