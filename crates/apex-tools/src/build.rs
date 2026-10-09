//! Project inspection and build/test execution.

use std::path::Path;
use std::time::Duration;

use apex_core::config::RiskClass;
use apex_core::error::{ApexError, Result};
use apex_core::process::{self, RunOptions};
use apex_protocol::ToolSpec;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::types::{truncate_content, Tool, ToolContext, ToolResult};

/// A detected project ecosystem and its conventional commands.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectKind {
    pub language: String,
    pub manifest: String,
    pub build_command: Option<String>,
    pub test_command: Option<String>,
}

/// Detect the project ecosystem in `root`.
pub fn detect(root: &Path) -> ProjectKind {
    let has = |name: &str| root.join(name).exists();
    if has("Cargo.toml") {
        ProjectKind {
            language: "rust".into(),
            manifest: "Cargo.toml".into(),
            build_command: Some("cargo build".into()),
            test_command: Some("cargo test".into()),
        }
    } else if has("go.mod") {
        ProjectKind {
            language: "go".into(),
            manifest: "go.mod".into(),
            build_command: Some("go build ./...".into()),
            test_command: Some("go test ./...".into()),
        }
    } else if has("package.json") {
        ProjectKind {
            language: "typescript/javascript".into(),
            manifest: "package.json".into(),
            build_command: Some("npm run build".into()),
            test_command: Some("npm test".into()),
        }
    } else if has("CMakeLists.txt") {
        ProjectKind {
            language: "c/c++".into(),
            manifest: "CMakeLists.txt".into(),
            build_command: Some("cmake --build .".into()),
            test_command: Some("ctest".into()),
        }
    } else if has("Makefile") || has("makefile") {
        ProjectKind {
            language: "make".into(),
            manifest: "Makefile".into(),
            build_command: Some("make".into()),
            test_command: Some("make test".into()),
        }
    } else if has("pyproject.toml") || has("requirements.txt") {
        ProjectKind {
            language: "python".into(),
            manifest: "pyproject.toml".into(),
            build_command: None,
            test_command: Some("pytest".into()),
        }
    } else {
        ProjectKind {
            language: "unknown".into(),
            manifest: String::new(),
            build_command: None,
            test_command: None,
        }
    }
}

async fn run_project_command(tool: &str, command: &str, ctx: &ToolContext) -> Result<ToolResult> {
    let parts = shell_words::split(command)
        .map_err(|e| ApexError::tool(tool, format!("could not parse command: {e}")))?;
    let (program, rest) = parts
        .split_first()
        .ok_or_else(|| ApexError::tool(tool, "empty command"))?;
    let options = RunOptions {
        cwd: ctx.workspace_root.clone(),
        timeout: Duration::from_secs(900),
        env: Vec::new(),
        stdin: None,
        max_output_bytes: process::DEFAULT_MAX_OUTPUT_BYTES,
    };
    let output = process::run(program, rest, &options).await?;
    let combined = format!(
        "$ {command}\nexit_code: {}\ntimed_out: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "none".into()),
        output.timed_out,
        if output.stdout.is_empty() {
            "(empty)"
        } else {
            &output.stdout
        },
        if output.stderr.is_empty() {
            "(empty)"
        } else {
            &output.stderr
        },
    );
    let (content, truncated) = truncate_content(&combined, ctx.max_content_bytes());
    let summary = if output.timed_out {
        format!("`{command}` timed out")
    } else if output.success {
        format!("`{command}` passed")
    } else {
        format!(
            "`{command}` failed (exit {})",
            output
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".into())
        )
    };
    Ok(ToolResult {
        ok: output.success,
        summary,
        content,
        data: json!({
            "command": command,
            "exit_code": output.exit_code,
            "success": output.success,
            "timed_out": output.timed_out
        }),
        truncated,
    })
}

/// Report detected project metadata.
pub struct ProjectInfoTool;

#[async_trait]
impl Tool for ProjectInfoTool {
    fn name(&self) -> &str {
        "project_info"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Detect the project language and its conventional build/test commands."
                .into(),
            parameters: json!({"type": "object", "properties": {}}),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::ReadOnly
    }

    async fn execute(&self, _args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let kind = detect(&ctx.workspace_root);
        let content = format!(
            "language: {}\nmanifest: {}\nbuild: {}\ntest: {}",
            kind.language,
            if kind.manifest.is_empty() {
                "(none)"
            } else {
                &kind.manifest
            },
            kind.build_command.as_deref().unwrap_or("(none)"),
            kind.test_command.as_deref().unwrap_or("(none)"),
        );
        Ok(ToolResult::success_with_data(
            format!("project: {}", kind.language),
            content,
            serde_json::to_value(&kind).unwrap_or(Value::Null),
        ))
    }
}

/// Build the project using its detected command.
pub struct RunBuildTool;

#[async_trait]
impl Tool for RunBuildTool {
    fn name(&self) -> &str {
        "run_build"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Build the project using its detected build command. Returns the real compiler output.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Override the detected build command."}
                }
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::Executing
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => detect(&ctx.workspace_root)
                .build_command
                .ok_or_else(|| ApexError::tool(self.name(), "no build command detected"))?,
        };
        run_project_command(self.name(), &command, ctx).await
    }
}

/// Test the project using its detected command.
pub struct RunTestsTool;

#[async_trait]
impl Tool for RunTestsTool {
    fn name(&self) -> &str {
        "run_tests"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Run the project's test suite using its detected test command. Returns the real test output.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Override the detected test command."}
                }
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::Executing
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => detect(&ctx.workspace_root)
                .test_command
                .ok_or_else(|| ApexError::tool(self.name(), "no test command detected"))?,
        };
        run_project_command(self.name(), &command, ctx).await
    }
}
