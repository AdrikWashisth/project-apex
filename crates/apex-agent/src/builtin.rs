//! Built-in agent definitions shipped with APEX.

use crate::manifest::{AgentManifest, MemoryPrefs, ModelPrefs, ToolPrefs};

const DEFAULT_INSTRUCTIONS: &str = r#"You are APEX, an autonomous software engineering agent operating inside a real Git repository.

Operating rules:
1. Inspect before you change. Use read_file, list_dir, glob_files and grep to understand the code first.
2. Make focused, minimal edits that satisfy the objective. Prefer edit_file for surgical changes.
3. After changing code, run the project's real build and tests (run_build, run_tests) and read the actual output.
4. Never claim a change works without evidence. Quote real command output; do not invent results.
5. If a build or test fails, diagnose the specific error and repair it. Do not retry blindly.
6. If you cannot complete the task, say so clearly and explain what is blocking you.
7. Confine all work to the workspace. Do not attempt to modify the .git directory.
8. When finished, state what you changed and the exact verification results."#;

/// The general-purpose default engineer agent.
pub fn default_agent() -> AgentManifest {
    AgentManifest {
        id: "apex-default".into(),
        name: "APEX Default Engineer".into(),
        version: "0.1.0".into(),
        description: "General-purpose coding agent that inspects, edits, builds and tests.".into(),
        maintainer: "APEX".into(),
        instructions: DEFAULT_INSTRUCTIONS.into(),
        task_types: vec![
            "feature".into(),
            "bugfix".into(),
            "refactor".into(),
            "task".into(),
        ],
        models: ModelPrefs::default(),
        tools: ToolPrefs::default(),
        execution: Default::default(),
        memory: MemoryPrefs::default(),
        evaluation: vec!["build-and-test".into()],
    }
}

/// A read-only reviewer agent that produces findings without editing.
pub fn reviewer_agent() -> AgentManifest {
    let mut manifest = default_agent();
    manifest.id = "apex-reviewer".into();
    manifest.name = "APEX Reviewer".into();
    manifest.description = "Read-only reviewer that inspects code and reports findings.".into();
    manifest.instructions = r#"You are APEX Reviewer, a meticulous code reviewer.

You inspect code and report findings. You do not modify files.
Use read_file, list_dir, glob_files, grep, git_status and git_diff to understand the change.
Report concrete issues with file paths and line numbers, ordered by severity. Be concise and specific."#
        .into();
    manifest.tools.allowed = vec![
        "read_file".into(),
        "list_dir".into(),
        "glob_files".into(),
        "grep".into(),
        "git_status".into(),
        "git_diff".into(),
        "project_info".into(),
    ];
    manifest.tools.denied = vec![
        "write_file".into(),
        "edit_file".into(),
        "run_command".into(),
        "run_build".into(),
        "run_tests".into(),
    ];
    manifest
}

/// A debugging agent focused on reproducing and fixing failures.
pub fn debugger_agent() -> AgentManifest {
    let mut manifest = default_agent();
    manifest.id = "apex-debugger".into();
    manifest.name = "APEX Debugger".into();
    manifest.description = "Reproduces failures, diagnoses root causes and repairs them.".into();
    manifest.instructions = r#"You are APEX Debugger. Your goal is to reproduce a failure, find its root cause, and fix it.

Method:
1. Reproduce the failure with a real command (run_tests or run_command). Capture the error.
2. Read the relevant files and form a specific hypothesis about the cause.
3. Apply the smallest fix that addresses the root cause.
4. Re-run the failing command and report the actual result.
Do not guess. Every conclusion must be backed by observed output."#
        .into();
    manifest
}

/// All built-in agents.
pub fn builtin_agents() -> Vec<AgentManifest> {
    vec![default_agent(), reviewer_agent(), debugger_agent()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_agents_validate() {
        for agent in builtin_agents() {
            agent.validate().unwrap();
        }
    }

    #[test]
    fn reviewer_cannot_edit() {
        let reviewer = reviewer_agent();
        assert!(!reviewer.allows_tool("edit_file"));
        assert!(reviewer.allows_tool("read_file"));
    }
}
