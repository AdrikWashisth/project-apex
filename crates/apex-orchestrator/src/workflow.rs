//! Converting workflow definitions into validated plans.
//!
//! The definition types live in `apex-protocol`; this module holds the logic
//! that resolves them against the agent catalog, which is what actually
//! validates an agent reference.

use apex_agent::AgentCatalog;
use apex_core::error::{ApexError, Result};
use apex_protocol::{Plan, PlannedStep, WorkflowDefinition};

use crate::team::{agent_can_write, validate_plan_agents};

/// Convert a workflow definition into a validated, executable plan.
///
/// Every agent reference is checked against the catalog, so an unknown agent
/// fails before the task starts rather than partway through it.
pub fn workflow_to_plan(workflow: &WorkflowDefinition, catalog: &AgentCatalog) -> Result<Plan> {
    workflow.validate()?;
    let mut steps = Vec::with_capacity(workflow.steps.len());
    for step in &workflow.steps {
        let manifest = catalog.get(&step.agent).ok_or_else(|| {
            let known: Vec<&str> = catalog.all().iter().map(|a| a.id.as_str()).collect();
            ApexError::config(format!(
                "workflow '{}' step '{}' references unknown agent '{}'; available agents: {}",
                workflow.name,
                step.id,
                step.agent,
                known.join(", ")
            ))
        })?;
        steps.push(PlannedStep {
            id: step.id.clone(),
            agent_id: step.agent.clone(),
            objective: step.objective.clone(),
            depends_on: step.depends_on.clone(),
            writes: step.writes.clone(),
            model: step.model.clone(),
            // Derive write capability from the manifest rather than trusting the
            // workflow file: a workflow cannot grant an agent more power than
            // its own manifest allows.
            read_only: !agent_can_write(manifest),
        });
    }
    let plan = Plan::new(steps);
    validate_plan_agents(&plan, catalog)?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REVIEW_THEN_FIX: &str = r#"
name = "review-then-fix"
description = "Review the working tree, then fix what the review found."

[[steps]]
id = "review"
agent = "apex-reviewer"
objective = "Review the current diff and report concrete problems."

[[steps]]
id = "fix"
agent = "apex-debugger"
objective = "Fix the problems the review identified."
depends_on = ["review"]
"#;

    fn catalog() -> AgentCatalog {
        AgentCatalog::load(None).unwrap()
    }

    #[test]
    fn parses_and_plans() {
        let workflow = WorkflowDefinition::from_toml(REVIEW_THEN_FIX).unwrap();
        let plan = workflow_to_plan(&workflow, &catalog()).unwrap();
        assert_eq!(plan.len(), 2);
        assert!(
            plan.step("review").unwrap().read_only,
            "reviewer cannot write"
        );
        assert!(!plan.step("fix").unwrap().read_only, "debugger can write");
        assert_eq!(plan.step("fix").unwrap().depends_on, vec!["review"]);
    }

    #[test]
    fn schedules_review_then_fix_into_two_waves() {
        let workflow = WorkflowDefinition::from_toml(REVIEW_THEN_FIX).unwrap();
        let plan = workflow_to_plan(&workflow, &catalog()).unwrap();
        let waves = crate::schedule(&plan, Default::default()).unwrap();
        assert_eq!(waves.len(), 2);
    }

    #[test]
    fn rejects_unknown_agent() {
        let bad = r#"
name = "bad"
[[steps]]
id = "s"
agent = "ghost"
objective = "x"
"#;
        let workflow = WorkflowDefinition::from_toml(bad).unwrap();
        let err = workflow_to_plan(&workflow, &catalog())
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown agent"), "got {err}");
    }

    #[test]
    fn rejects_cycle_in_workflow() {
        let bad = r#"
name = "bad"
[[steps]]
id = "a"
agent = "apex-default"
objective = "x"
depends_on = ["b"]

[[steps]]
id = "b"
agent = "apex-default"
objective = "y"
depends_on = ["a"]
"#;
        let workflow = WorkflowDefinition::from_toml(bad).unwrap();
        let err = workflow_to_plan(&workflow, &catalog())
            .unwrap_err()
            .to_string();
        assert!(err.contains("cycle"), "got {err}");
    }

    #[test]
    fn workflow_cannot_grant_more_power_than_the_manifest() {
        // The workflow claims the reviewer may write; the manifest forbids it,
        // so the derived step must stay read-only.
        let sneaky = r#"
name = "sneaky"
[[steps]]
id = "edit"
agent = "apex-reviewer"
objective = "Rewrite the source file."
writes = ["src/main.rs"]
"#;
        let workflow = WorkflowDefinition::from_toml(sneaky).unwrap();
        let plan = workflow_to_plan(&workflow, &catalog()).unwrap();
        assert!(
            plan.step("edit").unwrap().read_only,
            "a workflow must not make a read-only agent writable"
        );
    }
}
