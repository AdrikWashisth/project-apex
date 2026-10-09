//! Independent verification: build, test, diff and artefact checks.

use std::path::Path;
use std::time::Duration;

use apex_core::error::Result;
use apex_core::process::{self, RunOptions};
use apex_core::{git, paths};
use apex_protocol::CheckResult;
use serde::{Deserialize, Serialize};

use crate::contract::OutcomeContract;

/// The outcome of verifying a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    /// Whether every required check passed.
    pub passed: bool,
    /// Individual check results.
    pub checks: Vec<CheckResult>,
    /// A short summary of the diff.
    pub diff_summary: String,
    /// Files changed relative to HEAD.
    pub changed_files: Vec<String>,
}

impl VerificationReport {
    /// Failed, non-advisory checks.
    pub fn failures(&self) -> Vec<&CheckResult> {
        self.checks
            .iter()
            .filter(|c| !c.passed && !c.advisory)
            .collect()
    }
}

async fn run_check_command(
    name: &str,
    command: &str,
    workspace_root: &Path,
    timeout_secs: u64,
) -> CheckResult {
    let parts = match shell_words::split(command) {
        Ok(p) => p,
        Err(e) => {
            return CheckResult {
                name: name.into(),
                passed: false,
                detail: format!("could not parse command: {e}"),
                advisory: false,
            }
        }
    };
    let (program, rest) = match parts.split_first() {
        Some(p) => p,
        None => {
            return CheckResult {
                name: name.into(),
                passed: false,
                detail: "empty command".into(),
                advisory: false,
            }
        }
    };
    let options = RunOptions {
        cwd: workspace_root.to_path_buf(),
        timeout: Duration::from_secs(timeout_secs),
        env: Vec::new(),
        stdin: None,
        max_output_bytes: 256 * 1024,
    };
    match process::run(program, rest, &options).await {
        Ok(output) => {
            let detail =
                if output.success {
                    let tail = tail_lines(&output.stdout, 10);
                    format!("`{command}` exited 0\n{tail}")
                } else if output.timed_out {
                    format!("`{command}` timed out after {timeout_secs}s")
                } else {
                    let tail_out = tail_lines(&output.stdout, 20);
                    let tail_err = tail_lines(&output.stderr, 30);
                    format!(
                    "`{command}` exited {}\n--- stdout ---\n{tail_out}\n--- stderr ---\n{tail_err}",
                    output.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "none".into())
                )
                };
            CheckResult {
                name: name.into(),
                passed: output.success,
                detail,
                advisory: false,
            }
        }
        Err(e) => CheckResult {
            name: name.into(),
            passed: false,
            detail: format!("could not run `{command}`: {e}"),
            advisory: false,
        },
    }
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Verify a task against its outcome contract using observable evidence.
pub async fn verify(
    workspace_root: &Path,
    contract: &OutcomeContract,
) -> Result<VerificationReport> {
    contract.validate_artifacts(workspace_root)?;
    let mut checks: Vec<CheckResult> = Vec::new();

    // 1. Build.
    if let Some(command) = &contract.build_command {
        checks.push(run_check_command("build", command, workspace_root, 900).await);
    }

    // 2. Tests.
    if let Some(command) = &contract.test_command {
        checks.push(run_check_command("tests", command, workspace_root, 900).await);
    }

    // 3. Expected artifacts.
    for artifact in &contract.expected_artifacts {
        let path = paths::resolve_within(workspace_root, artifact)?;
        checks.push(CheckResult {
            name: format!("artifact:{artifact}"),
            passed: path.exists(),
            detail: if path.exists() {
                format!("{artifact} exists")
            } else {
                format!("{artifact} is missing")
            },
            advisory: false,
        });
    }

    // 4. Diff.
    let mut changed_files = Vec::new();
    let mut diff_summary = String::new();
    if git::is_repo(workspace_root).await.unwrap_or(false) {
        let status = git::status(workspace_root).await.unwrap_or_default();
        changed_files = status.iter().map(|e| e.path.clone()).collect();
        diff_summary = git::diff_stat(workspace_root).await.unwrap_or_default();
        if contract.require_diff {
            checks.push(CheckResult {
                name: "diff".into(),
                passed: !changed_files.is_empty(),
                detail: if changed_files.is_empty() {
                    "no files changed".into()
                } else {
                    format!(
                        "{} file(s) changed:\n{}",
                        changed_files.len(),
                        changed_files.join("\n")
                    )
                },
                advisory: false,
            });
        }
    } else if contract.require_diff {
        checks.push(CheckResult {
            name: "diff".into(),
            passed: false,
            detail: "workspace is not a Git repository; cannot verify a diff".into(),
            advisory: false,
        });
    }

    let passed = checks.iter().all(|c| c.passed || c.advisory);
    Ok(VerificationReport {
        passed,
        checks,
        diff_summary,
        changed_files,
    })
}

/// Build a repair instruction describing what failed, for the agent to fix.
pub fn repair_instruction(report: &VerificationReport, attempt: u32) -> String {
    let mut text = format!(
        "Verification failed (repair attempt {attempt}). Fix the underlying problems, then the task will be verified again.\n"
    );
    for failure in report.failures() {
        text.push_str(&format!(
            "\n## Failing check: {}\n{}\n",
            failure.name, failure.detail
        ));
    }
    text.push_str(
        "\nAddress the root cause of each failure. Do not disable or delete tests to make them pass. Re-run the relevant commands to confirm the fix.",
    );
    text
}
