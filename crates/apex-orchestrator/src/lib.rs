//! Planning and scheduling for multi-agent APEX tasks.
//!
//! This crate owns *scheduling*: it turns a [`Plan`] (a domain type from
//! `apex-protocol`) into ordered execution waves, and it builds plans from a
//! user-defined team or workflow. It deliberately knows nothing about how an
//! agent runs — the runtime owns execution.

pub mod scheduler;
pub mod team;
pub mod workflow;

pub use apex_protocol::{Plan, PlannedStep, WorkflowDefinition, WorkflowStep, MATCH_ALL};
pub use scheduler::{
    globs_conflict, schedule, steps_conflict, SchedulerConfig, DEFAULT_MAX_PARALLEL,
};
pub use team::{agent_can_write, plan_from_team, validate_plan_agents};
pub use workflow::workflow_to_plan;

use apex_agent::AgentCatalog;
use apex_core::error::Result;
use apex_protocol::{ExecutionMode, TeamSpec};

/// Build a plan for a task given its execution mode.
///
/// * `Single` needs no plan.
/// * `ManualMulti` needs a team.
/// * `Workflow` needs a workflow definition.
/// * `Orchestrated` needs an explicit plan (produced by the orchestrator).
pub fn plan_for_mode(
    mode: ExecutionMode,
    team: Option<&TeamSpec>,
    workflow: Option<&WorkflowDefinition>,
    plan: Option<&Plan>,
    objective: &str,
    catalog: &AgentCatalog,
) -> Result<Plan> {
    match mode {
        ExecutionMode::Single => Err(apex_core::error::ApexError::config(
            "single-agent mode does not use a multi-step plan",
        )),
        ExecutionMode::ManualMulti => {
            let team = team.ok_or_else(|| {
                apex_core::error::ApexError::config(
                    "manual multi-agent mode requires a team; pass `--agents a,b,c`",
                )
            })?;
            plan_from_team(team, objective, catalog)
        }
        ExecutionMode::Workflow => {
            let workflow = workflow.ok_or_else(|| {
                apex_core::error::ApexError::config(
                    "workflow mode requires a workflow file; pass `--workflow <file.toml>`",
                )
            })?;
            workflow_to_plan(workflow, catalog)
        }
        ExecutionMode::Orchestrated => {
            let plan = plan.ok_or_else(|| {
                apex_core::error::ApexError::config(
                    "orchestrated mode requires a plan; the orchestrator must produce one first",
                )
            })?;
            validate_plan_agents(plan, catalog)?;
            Ok(plan.clone())
        }
    }
}
