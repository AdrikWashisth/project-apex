//! Tool registry: registration, native specs, permission checks and dispatch.

use std::collections::BTreeMap;
use std::sync::Arc;

use apex_core::config::RiskClass;
use apex_core::error::{ApexError, Result};
use apex_protocol::ToolSpec;
use serde_json::Value;

use crate::build::{ProjectInfoTool, RunBuildTool, RunTestsTool};
use crate::exec::RunCommandTool;
use crate::fs::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
use crate::git_tools::{GitDiffTool, GitStatusTool};
use crate::search::{GlobFilesTool, GrepTool};
use crate::types::{Tool, ToolContext, ToolResult};

/// A registry of available tools.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        ToolRegistry {
            tools: BTreeMap::new(),
        }
    }

    /// Create a registry containing the built-in tool set.
    pub fn default_set() -> Self {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(WriteFileTool));
        registry.register(Arc::new(EditFileTool));
        registry.register(Arc::new(ListDirTool));
        registry.register(Arc::new(GlobFilesTool));
        registry.register(Arc::new(GrepTool));
        registry.register(Arc::new(RunCommandTool));
        registry.register(Arc::new(GitStatusTool));
        registry.register(Arc::new(GitDiffTool));
        registry.register(Arc::new(ProjectInfoTool));
        registry.register(Arc::new(RunBuildTool));
        registry.register(Arc::new(RunTestsTool));
        registry
    }

    /// Register a tool, replacing any existing tool with the same name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Whether a tool with the given name exists.
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Tool names, sorted.
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Risk class for a tool, if present.
    pub fn risk(&self, name: &str) -> Option<RiskClass> {
        self.tools.get(name).map(|t| t.risk())
    }

    /// Model-facing specs for every registered tool.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    /// Execute a tool by name, enforcing the context's permission policy.
    pub async fn execute(&self, name: &str, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ApexError::tool(name, "unknown tool"))?;
        if let Some(reason) = ctx.deny_reason(name, tool.risk()) {
            return Err(ApexError::Permission(reason));
        }
        tool.execute(args, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_core::config::PermissionProfile;

    #[test]
    fn default_set_has_core_tools() {
        let registry = ToolRegistry::default_set();
        for name in [
            "read_file",
            "write_file",
            "edit_file",
            "run_tests",
            "git_diff",
        ] {
            assert!(registry.contains(name), "missing tool {name}");
        }
    }

    #[tokio::test]
    async fn read_only_profile_blocks_writes() {
        let registry = ToolRegistry::default_set();
        let mut permissions = apex_core::config::Permissions::default();
        permissions.profile = PermissionProfile::ReadOnly;
        let ctx = ToolContext::new(std::env::temp_dir(), permissions);
        let err = registry
            .execute(
                "write_file",
                serde_json::json!({"path": "x", "content": "y"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ApexError::Permission(_)));
    }
}
