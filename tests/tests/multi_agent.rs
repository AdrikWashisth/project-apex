//! Integration tests for multi-agent execution (Milestone 5).
//!
//! These prove the real behaviour: a team runs, results are handed forward
//! between agents, read-only agents run concurrently, write conflicts are
//! serialised, budgets are shared, and workflows execute.

use std::sync::Arc;
use std::time::Duration;

use apex_core::config::{PermissionProfile, Permissions};
use apex_core::error::Result;
use apex_memory::Store;
use apex_protocol::{ExecutionMode, Plan, PlannedStep, SubtaskStatus, TeamSpec};
use apex_runtime::Runtime;
use apex_tests::support::{init_git_repo, offline_config};

/// Build a runtime whose fake provider completes instantly.
fn runtime() -> Result<Arc<Runtime>> {
    let mut config = offline_config();
    config.permissions = Permissions {
        profile: PermissionProfile::ControlledAutonomous,
        allow_shell: true,
        require_approval: Vec::new(),
        ..Default::default()
    };
    Runtime::new(config, Arc::new(Store::open_in_memory()?))
}

async fn wait_terminal(runtime: &Arc<Runtime>, task_id: &str) -> Result<apex_protocol::Task> {
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(task) = runtime.store.get_task(task_id)? {
            if task.status.is_terminal() {
                return Ok(task);
            }
        }
        if std::time::Instant::now() > deadline {
            return Err(apex_core::error::ApexError::Storage(format!(
                "timed out waiting for task {task_id}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_team_runs_and_records_subtasks() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();
    init_git_repo(&repo).await?;
    let runtime = runtime()?;

    let task = runtime.create_task_with_plan(
        "Review the code and report problems".into(),
        repo.to_string_lossy().into_owned(),
        None,
        None,
        ExecutionMode::ManualMulti,
        Some(apex_protocol::TeamSpec {
            agents: vec!["apex-reviewer".into(), "apex-debugger".into()],
            strategy: apex_protocol::TeamStrategy::Sequential,
            model: None,
            writes: None,
        }),
        None,
        None,
    )?;

    // The plan is built and stored up front.
    let plan = task.plan.clone().expect("a manual team produces a plan");
    assert_eq!(plan.len(), 2, "both team members should be in the plan");
    assert!(
        plan.step("apex-reviewer").unwrap().read_only,
        "the reviewer must be read-only"
    );
    assert!(
        !plan.step("apex-debugger").unwrap().read_only,
        "the debugger may modify files"
    );

    let finished = wait_terminal(&runtime, &task.id).await?;
    assert!(finished.status.is_terminal());

    let subtasks = runtime.store.list_subtasks(&task.id)?;
    assert_eq!(subtasks.len(), 2, "every plan step becomes a subtask");
    for subtask in &subtasks {
        assert!(
            matches!(
                subtask.status,
                SubtaskStatus::Completed | SubtaskStatus::Failed
            ),
            "subtask '{}' ended as {:?}",
            subtask.id,
            subtask.status
        );
        assert!(
            subtask.result.is_some(),
            "subtask '{}' should record the agent's answer",
            subtask.id
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subtask_events_are_persisted_for_replay() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();
    init_git_repo(&repo).await?;
    let runtime = runtime()?;

    let task = runtime.create_task_with_plan(
        "Do the thing".into(),
        repo.to_string_lossy().into_owned(),
        None,
        None,
        ExecutionMode::ManualMulti,
        Some(TeamSpec::new(["apex-default", "apex-reviewer"])),
        None,
        None,
    )?;
    wait_terminal(&runtime, &task.id).await?;

    let events = runtime.store.events_after(&task.id, 0)?;
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, apex_protocol::EventKind::SubtaskStarted { .. })),
        "subtask_started events must be persisted"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, apex_protocol::EventKind::SubtaskFinished { .. })),
        "subtask_finished events must be persisted"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, apex_protocol::EventKind::PlanScheduled { .. })),
        "the computed plan must be persisted as an event"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_definition_drives_execution() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();
    init_git_repo(&repo).await?;
    let runtime = runtime()?;

    let workflow = apex_orchestrator::WorkflowDefinition::from_toml(
        r#"
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
"#,
    )?;

    let task = runtime.create_task_with_plan(
        "Improve the code".into(),
        repo.to_string_lossy().into_owned(),
        None,
        None,
        ExecutionMode::Workflow,
        None,
        Some(workflow),
        None,
    )?;

    let plan = task.plan.clone().expect("a workflow produces a plan");
    assert_eq!(plan.len(), 2);
    assert_eq!(
        plan.step("fix").unwrap().depends_on,
        vec!["review".to_string()],
        "the fix step depends on the review step"
    );

    let waves = apex_orchestrator::schedule(&plan, Default::default())?;
    assert_eq!(waves.len(), 2, "review must finish before fix starts");

    let finished = wait_terminal(&runtime, &task.id).await?;
    assert!(finished.status.is_terminal());
    assert_eq!(runtime.store.list_subtasks(&task.id)?.len(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn team_runs_as_waves_when_agents_conflict() -> Result<()> {
    // Two agents that can both write the whole workspace must not run at the
    // same time; the scheduler must serialise them into separate waves.
    let plan = Plan::new(vec![
        PlannedStep::new("writer-a", "apex-default", "edit files"),
        PlannedStep::new("writer-b", "apex-debugger", "edit files"),
    ]);
    let waves = apex_orchestrator::schedule(&plan, Default::default())?;
    assert_eq!(
        waves.len(),
        2,
        "two undeclared writers must never share a wave"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_only_team_runs_in_a_single_wave() -> Result<()> {
    let plan = Plan::new(vec![
        PlannedStep::new("r1", "apex-reviewer", "review").read_only(true),
        PlannedStep::new("r2", "apex-reviewer", "review again").read_only(true),
    ]);
    let waves = apex_orchestrator::schedule(&plan, Default::default())?;
    assert_eq!(
        waves.len(),
        1,
        "read-only agents cannot conflict and should run concurrently"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_team_without_write_scope_is_refused() -> Result<()> {
    // Two write-capable agents with no declared scope must not be allowed to
    // "run in parallel" — the scheduler would serialise them anyway, so the
    // honest answer is to refuse.
    let catalog = apex_agent::AgentCatalog::load(None)?;
    let spec = apex_protocol::TeamSpec {
        agents: vec!["apex-default".into(), "apex-debugger".into()],
        strategy: apex_protocol::TeamStrategy::Parallel,
        model: None,
        writes: None,
    };
    let err = apex_orchestrator::plan_from_team(&spec, "x", &catalog)
        .expect_err("an unscoped parallel team must be refused");
    let message = err.to_string();
    assert!(
        message.contains("cannot run these team members in parallel"),
        "got: {message}"
    );
    assert!(
        message.contains("apex-default"),
        "the error should name the offending member, got: {message}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parallel_team_with_a_shared_scope_runs_in_one_wave() -> Result<()> {
    // A team's --writes applies to every member, so a shared scope makes the
    // whole team one wave.
    let catalog = apex_agent::AgentCatalog::load(None)?;
    let spec = TeamSpec {
        agents: vec!["apex-reviewer".into(), "apex-reviewer".into()],
        strategy: apex_protocol::TeamStrategy::Parallel,
        model: None,
        writes: Some(vec!["src/**".into()]),
    };
    let plan = apex_orchestrator::plan_from_team(&spec, "x", &catalog)?;
    let waves = apex_orchestrator::schedule(&plan, Default::default())?;
    assert_eq!(waves.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disjoint_scopes_make_a_team_genuinely_parallel() -> Result<()> {
    // Per-step scopes come from a workflow. Provably disjoint scopes are what
    // actually collapses two write-capable agents into a single wave.
    let catalog = apex_agent::AgentCatalog::load(None)?;
    let workflow = apex_orchestrator::WorkflowDefinition::from_toml(
        r#"
name = "disjoint"
description = "Two writers in different directories."

[[steps]]
id = "src-work"
agent = "apex-default"
objective = "Work on the source tree."
writes = ["src/**"]

[[steps]]
id = "docs-work"
agent = "apex-debugger"
objective = "Work on the docs tree."
writes = ["docs/**"]
"#,
    )?;
    let plan = apex_orchestrator::workflow_to_plan(&workflow, &catalog)?;
    let waves = apex_orchestrator::schedule(&plan, Default::default())?;
    assert_eq!(
        waves.len(),
        1,
        "provably disjoint write scopes must run concurrently"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_team_with_overlapping_scopes_is_refused() -> Result<()> {
    let catalog = apex_agent::AgentCatalog::load(None)?;
    let spec = apex_protocol::TeamSpec {
        agents: vec!["apex-default".into(), "apex-debugger".into()],
        strategy: apex_protocol::TeamStrategy::Parallel,
        model: None,
        writes: Some(vec!["src/**".into()]),
    };
    let err = apex_orchestrator::plan_from_team(&spec, "x", &catalog)
        .expect_err("overlapping scopes must be refused");
    assert!(
        err.to_string().contains("overlapping write scopes"),
        "got: {err}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_parallel_team_needs_no_scope() -> Result<()> {
    // Read-only agents can never conflict, so no scope is required.
    let catalog = apex_agent::AgentCatalog::load(None)?;
    let spec = apex_protocol::TeamSpec {
        agents: vec!["apex-reviewer".into(), "apex-reviewer".into()],
        strategy: apex_protocol::TeamStrategy::Parallel,
        model: None,
        writes: None,
    };
    let plan = apex_orchestrator::plan_from_team(&spec, "x", &catalog)?;
    assert_eq!(plan.len(), 2);
    let waves = apex_orchestrator::schedule(&plan, Default::default())?;
    assert_eq!(waves.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_budget_bounds_a_fan_out() -> Result<()> {
    // A team must not multiply the parent task's spend.
    let budget = apex_core::config::Budget {
        max_tool_calls: 6,
        ..Default::default()
    };
    let tracker = apex_runtime::multi::BudgetTracker::new(budget);

    let slice = tracker.slice_for_subtask(3);
    assert_eq!(
        slice.max_tool_calls, 2,
        "each of three concurrent subtasks gets a third of the remaining calls"
    );
    assert_eq!(
        slice.max_repair_attempts, 0,
        "repairs happen once at the task level, not per subtask"
    );
    Ok(())
}
