//! Turning a user-assembled team into a validated plan.
//!
//! The user names the agents; APEX works out the rest. Read-only agents
//! (derived from their manifest's tool permissions) may run concurrently with
//! each other, while agents that can modify the workspace are serialised
//! unless their write scopes are provably disjoint.

use apex_agent::{AgentCatalog, AgentManifest};
use apex_core::error::{ApexError, Result};
use apex_protocol::{Plan, PlannedStep, TeamSpec, TeamStrategy};

/// Tools that can modify the workspace or run commands.
const MUTATING_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "run_command",
    "run_build",
    "run_tests",
];

/// Whether an agent's manifest permits modifying the workspace.
pub fn agent_can_write(manifest: &AgentManifest) -> bool {
    MUTATING_TOOLS.iter().any(|tool| manifest.allows_tool(tool))
}

/// Validate a plan and confirm every referenced agent exists.
pub fn validate_plan_agents(plan: &Plan, catalog: &AgentCatalog) -> Result<()> {
    plan.validate()?;
    for step in &plan.steps {
        if catalog.get(&step.agent_id).is_none() {
            return Err(unknown_agent(&step.agent_id, catalog));
        }
    }
    Ok(())
}

fn unknown_agent(agent_id: &str, catalog: &AgentCatalog) -> ApexError {
    let known: Vec<&str> = catalog.all().iter().map(|a| a.id.as_str()).collect();
    ApexError::config(format!(
        "unknown agent '{agent_id}'; available agents: {}",
        known.join(", ")
    ))
}

/// Frame the shared objective for one team member.
///
/// Each agent keeps its own identity and instructions; the framing only tells
/// it what the team is trying to achieve and who else is involved.
fn objective_for(
    manifest: &AgentManifest,
    objective: &str,
    position: usize,
    total: usize,
) -> String {
    let role = if agent_can_write(manifest) {
        "You may modify the workspace as part of this task."
    } else {
        "You must not modify the workspace; report findings instead."
    };
    format!(
        "{objective}\n\nYou are member {position} of {total} on this team, \
         acting as {} ({}).\n{role}\n\
         If a previous team member already produced findings, build on them \
         rather than repeating the work.",
        manifest.name, manifest.id
    )
}

/// Build a validated plan from a team specification.
pub fn plan_from_team(spec: &TeamSpec, objective: &str, catalog: &AgentCatalog) -> Result<Plan> {
    if spec.agents.is_empty() {
        return Err(ApexError::config(
            "a team needs at least one agent; try `--agents apex-debugger`",
        ));
    }

    let total = spec.agents.len();
    let mut steps: Vec<PlannedStep> = Vec::with_capacity(total);
    let mut previous: Option<String> = None;

    for (index, agent_id) in spec.agents.iter().enumerate() {
        let manifest = catalog
            .get(agent_id)
            .ok_or_else(|| unknown_agent(agent_id, catalog))?;

        let mut step = PlannedStep::new(
            agent_id.clone(),
            agent_id.clone(),
            objective_for(manifest, objective, index + 1, total),
        )
        .read_only(!agent_can_write(manifest));
        step.model = spec.model.clone();

        // In sequential mode each agent depends on the one before it, which is
        // also how findings are handed forward.
        if spec.strategy == TeamStrategy::Sequential {
            if let Some(prev) = &previous {
                step = step.depending_on(vec![prev.clone()]);
            }
        }
        previous = Some(agent_id.clone());
        steps.push(step);
    }

    let plan = Plan::new(steps);
    validate_plan_agents(&plan, catalog)?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> AgentCatalog {
        AgentCatalog::load(None).unwrap()
    }

    #[test]
    fn reviewer_is_treated_as_read_only() {
        let cat = catalog();
        assert!(!agent_can_write(cat.get("apex-reviewer").unwrap()));
        assert!(agent_can_write(cat.get("apex-debugger").unwrap()));
    }

    #[test]
    fn sequential_team_chains_dependencies() {
        let plan = plan_from_team(
            &TeamSpec::new(["apex-default", "apex-reviewer"]),
            "do the thing",
            &catalog(),
        )
        .unwrap();
        assert_eq!(plan.len(), 2);
        assert!(plan.step("apex-default").unwrap().depends_on.is_empty());
        assert_eq!(
            plan.step("apex-reviewer").unwrap().depends_on,
            vec!["apex-default".to_string()]
        );
    }

    #[test]
    fn parallel_team_has_no_dependencies() {
        let spec = TeamSpec {
            agents: vec!["apex-default".into(), "apex-reviewer".into()],
            strategy: TeamStrategy::Parallel,
            model: None,
        };
        let plan = plan_from_team(&spec, "do the thing", &catalog()).unwrap();
        assert!(plan.steps.iter().all(|s| s.depends_on.is_empty()));
    }

    #[test]
    fn rejects_unknown_agent() {
        let err = plan_from_team(&TeamSpec::new(["not-a-real-agent"]), "x", &catalog())
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown agent"), "got {err}");
    }

    #[test]
    fn rejects_empty_team() {
        let empty: Vec<String> = Vec::new();
        assert!(plan_from_team(&TeamSpec::new(empty), "x", &catalog()).is_err());
    }

    #[test]
    fn reviewer_step_is_marked_read_only() {
        let plan = plan_from_team(&TeamSpec::new(["apex-reviewer"]), "x", &catalog()).unwrap();
        assert!(plan.step("apex-reviewer").unwrap().read_only);
    }

    #[test]
    fn model_override_is_applied_to_every_step() {
        let spec = TeamSpec {
            agents: vec!["apex-default".into()],
            strategy: TeamStrategy::Sequential,
            model: Some("openai/gpt-4o".into()),
        };
        let plan = plan_from_team(&spec, "x", &catalog()).unwrap();
        assert_eq!(
            plan.steps[0].model.as_deref(),
            Some("openai/gpt-4o"),
            "the team model override should reach each member"
        );
    }
}
