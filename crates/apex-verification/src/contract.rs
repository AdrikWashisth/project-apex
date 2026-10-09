//! Outcome contracts: what "done" means for a task.

use apex_core::paths::resolve_within;
use apex_tools::build::detect;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// An explicit, checkable contract for a task's completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeContract {
    /// The objective, restated plainly.
    pub objective: String,
    /// Constraints the solution must respect.
    #[serde(default)]
    pub constraints: Vec<String>,
    /// Human-readable acceptance criteria.
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    /// Files expected to exist after the task (relative paths).
    #[serde(default)]
    pub expected_artifacts: Vec<String>,
    /// Build command to run, if any.
    #[serde(default)]
    pub build_command: Option<String>,
    /// Test command to run, if any.
    #[serde(default)]
    pub test_command: Option<String>,
    /// Whether a non-empty diff is required.
    #[serde(default)]
    pub require_diff: bool,
}

impl OutcomeContract {
    /// Derive a sensible default contract from an objective and the workspace.
    pub fn derive(objective: impl Into<String>, workspace_root: &Path) -> Self {
        let kind = detect(workspace_root);
        OutcomeContract {
            objective: objective.into(),
            constraints: vec!["Stay within the workspace root.".into()],
            acceptance_criteria: vec![
                "The project builds successfully.".into(),
                "The project's tests pass.".into(),
            ],
            expected_artifacts: Vec::new(),
            build_command: kind.build_command,
            test_command: kind.test_command,
            require_diff: true,
        }
    }

    /// Validate that artefact paths stay inside the workspace.
    pub fn validate_artifacts(&self, workspace_root: &Path) -> apex_core::error::Result<()> {
        for artifact in &self.expected_artifacts {
            resolve_within(workspace_root, artifact)?;
        }
        Ok(())
    }
}
