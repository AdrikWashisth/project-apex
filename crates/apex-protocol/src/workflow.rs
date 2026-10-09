//! User-defined workflow definitions.
//!
//! A workflow turns a standing procedure into an ordered set of agent steps.
//! It is data, not behaviour, so it lives here and can travel over the wire;
//! converting it into an executable plan (which needs the agent catalog) lives
//! in `apex-orchestrator`.

use apex_core::error::{ApexError, Result};
use serde::{Deserialize, Serialize};

/// A single step in a workflow file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Unique id within the workflow.
    pub id: String,
    /// Agent that performs the step.
    pub agent: String,
    /// What the step must accomplish.
    pub objective: String,
    /// Steps that must finish first.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Paths this step may modify. Omit to allow the whole workspace.
    #[serde(default)]
    pub writes: Vec<String>,
    /// Model override for this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<WorkflowStep>,
}

impl WorkflowDefinition {
    /// Parse a workflow from TOML text.
    pub fn from_toml(text: &str) -> Result<WorkflowDefinition> {
        let definition: WorkflowDefinition = toml::from_str(text)
            .map_err(|e| ApexError::config(format!("invalid workflow definition: {e}")))?;
        definition.validate()?;
        Ok(definition)
    }

    /// Load a workflow from a file.
    pub fn load(path: &std::path::Path) -> Result<WorkflowDefinition> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            ApexError::config(format!("could not read workflow {}: {e}", path.display()))
        })?;
        WorkflowDefinition::from_toml(&text)
    }

    /// Basic structural validation.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(ApexError::config("workflow requires a non-empty 'name'"));
        }
        if self.steps.is_empty() {
            return Err(ApexError::config(format!(
                "workflow '{}' defines no steps",
                self.name
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
name = "review-then-fix"
description = "Review, then fix."

[[steps]]
id = "review"
agent = "apex-reviewer"
objective = "Review the diff."

[[steps]]
id = "fix"
agent = "apex-debugger"
objective = "Fix what the review found."
depends_on = ["review"]
"#;

    #[test]
    fn parses_a_workflow() {
        let workflow = WorkflowDefinition::from_toml(SAMPLE).unwrap();
        assert_eq!(workflow.name, "review-then-fix");
        assert_eq!(workflow.steps.len(), 2);
        assert_eq!(workflow.steps[1].depends_on, vec!["review"]);
    }

    #[test]
    fn rejects_a_workflow_without_a_name() {
        assert!(WorkflowDefinition::from_toml("steps = []").is_err());
    }

    #[test]
    fn rejects_a_workflow_without_steps() {
        assert!(WorkflowDefinition::from_toml(r#"name = "empty""#).is_err());
    }
}
