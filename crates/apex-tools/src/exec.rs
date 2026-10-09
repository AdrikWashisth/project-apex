//! Shell/command execution tool (bounded, no implicit shell).

use std::time::Duration;

use apex_core::config::RiskClass;
use apex_core::error::{ApexError, Result};
use apex_core::paths::resolve_within;
use apex_core::process::{self, RunOptions, DEFAULT_MAX_OUTPUT_BYTES};
use apex_protocol::ToolSpec;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::types::{truncate_content, Tool, ToolContext, ToolResult};

/// Run an executable with arguments. No shell is used, so pipes, redirection
/// and shell builtins are not available. Use dedicated build/test tools for
/// project commands.
pub struct RunCommandTool;

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Run a program with arguments in the workspace (no shell features). Returns exit code, stdout and stderr.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Command line, e.g. 'cargo build --quiet'. Parsed into program + args."},
                    "cwd": {"type": "string", "description": "Working directory relative to the workspace root."},
                    "timeout_secs": {"type": "integer", "description": "Timeout in seconds (default 120)."}
                },
                "required": ["command"]
            }),
        }
    }

    fn risk(&self) -> RiskClass {
        RiskClass::Executing
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let command_line = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApexError::tool(self.name(), "missing 'command'"))?;
        let parts = shell_words::split(command_line)
            .map_err(|e| ApexError::tool(self.name(), format!("could not parse command: {e}")))?;
        let (program, rest) = parts
            .split_first()
            .ok_or_else(|| ApexError::tool(self.name(), "empty command"))?;

        let cwd = resolve_within(
            &ctx.workspace_root,
            args.get("cwd").and_then(|v| v.as_str()).unwrap_or("."),
        )?;
        if !cwd.exists() {
            return Err(ApexError::tool(
                self.name(),
                format!("working directory does not exist: {}", cwd.display()),
            ));
        }

        let timeout = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(120)
            .clamp(1, 3600);

        let options = RunOptions {
            cwd,
            timeout: Duration::from_secs(timeout),
            env: Vec::new(),
            stdin: None,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        };

        let output = process::run(program, rest, &options).await?;
        let combined = format!(
            "exit_code: {}\ntimed_out: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
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
            format!("'{command_line}' timed out after {timeout}s")
        } else if output.success {
            format!("'{command_line}' exited 0")
        } else {
            format!(
                "'{command_line}' exited {}",
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
                "exit_code": output.exit_code,
                "success": output.success,
                "timed_out": output.timed_out
            }),
            truncated,
        })
    }
}
