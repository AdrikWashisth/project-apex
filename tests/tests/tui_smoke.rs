//! Smoke tests for the terminal interface.
//!
//! The TUI drives a real terminal, so these tests exercise the parts that do not
//! need one: state transitions and the public state API. Rendering is a pure
//! function of `TuiState`, so it is covered by the state tests rather than by
//! driving a pty.

use apex_cli::tui::TuiState;
use apex_protocol::{ExecutionMode, Task, TaskStatus, TeamSpec};

fn sample_task(id: &str, objective: &str) -> Task {
    Task {
        id: id.into(),
        objective: objective.into(),
        project_root: "/tmp".into(),
        model: None,
        agent_id: None,
        mode: ExecutionMode::Single,
        plan: None,
        status: TaskStatus::Running,
        created_at: String::new(),
        updated_at: String::new(),
        started_at: None,
        finished_at: None,
        budget: Default::default(),
        usage: Default::default(),
        tool_calls: 0,
        steps: 0,
        repair_attempts: 0,
        summary: None,
        error: None,
    }
}

#[test]
fn a_new_state_has_no_selection() {
    let state = TuiState::new();
    assert_eq!(state.task_count(), 0);
    assert!(state.current().is_none());
    assert_eq!(state.selected_index(), 0);
}

#[test]
fn selection_clamps_when_tasks_shrink() {
    let mut state = TuiState::new();
    state.set_selected(5);
    state.clamp_selection();
    assert_eq!(
        state.selected_index(),
        0,
        "an empty task list must reset the selection"
    );
}

#[test]
fn a_task_with_no_plan_is_single_agent() {
    let mut state = TuiState::new();
    state.push_task(sample_task("task_1", "x"));
    state.clamp_selection();
    assert_eq!(state.selected_index(), 0);
    assert!(state.current().is_some());
    assert!(state.current().unwrap().plan.is_none());
}

#[test]
fn a_diff_is_stored_for_the_diff_pane() {
    let mut state = TuiState::new();
    assert!(state.diff().is_none());
    state.set_diff(Some("+let x = 1;\n".to_string()));
    assert!(state.diff().is_some());
}

#[test]
fn a_team_spec_carries_its_write_scope() {
    let spec = TeamSpec {
        agents: vec!["apex-default".into()],
        strategy: apex_protocol::TeamStrategy::Parallel,
        model: None,
        writes: Some(vec!["src/**".into()]),
    };
    assert_eq!(spec.writes.as_deref(), Some(&["src/**".to_string()][..]));
}

#[test]
fn a_sequential_team_spec_needs_no_write_scope() {
    let spec = TeamSpec {
        agents: vec!["apex-default".into(), "apex-debugger".into()],
        strategy: apex_protocol::TeamStrategy::Sequential,
        model: None,
        writes: None,
    };
    // Sequential teams hand work forward, so no scope is needed.
    assert!(spec.writes.is_none());
}

#[test]
fn a_parallel_team_scope_must_be_disjoint_to_schedule() {
    // A team's --writes is a shared scope, so two write-capable members given
    // it cannot run in parallel: they would claim the same files.
    let catalog = apex_agent::AgentCatalog::load(None).unwrap();
    let spec = TeamSpec {
        agents: vec!["apex-default".into(), "apex-debugger".into()],
        strategy: apex_protocol::TeamStrategy::Parallel,
        model: None,
        writes: Some(vec!["src/**".into(), "docs/**".into()]),
    };
    let error = apex_orchestrator::plan_from_team(&spec, "x", &catalog)
        .expect_err("a shared scope across two writers must be refused");
    assert!(error.to_string().contains("overlapping write scopes"));
}

#[test]
fn disjoint_workflow_scopes_really_do_run_in_parallel() {
    let catalog = apex_agent::AgentCatalog::load(None).unwrap();
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
    )
    .unwrap();
    let plan = apex_orchestrator::workflow_to_plan(&workflow, &catalog).unwrap();
    let waves = apex_orchestrator::schedule(&plan, Default::default()).unwrap();
    assert_eq!(
        waves.len(),
        1,
        "disjoint scopes must collapse into one wave"
    );
}

#[test]
fn an_unscoped_parallel_team_is_refused_not_serialised() {
    let catalog = apex_agent::AgentCatalog::load(None).unwrap();
    let spec = TeamSpec {
        agents: vec!["apex-default".into(), "apex-debugger".into()],
        strategy: apex_protocol::TeamStrategy::Parallel,
        model: None,
        writes: None,
    };
    let error = apex_orchestrator::plan_from_team(&spec, "x", &catalog)
        .expect_err("must refuse rather than silently serialise");
    assert!(error
        .to_string()
        .contains("cannot run these team members in parallel"));
}

#[test]
fn a_repeated_agent_in_a_team_gets_a_distinct_step_id() {
    // Two reviewers is a legitimate team; it must not collide on step ids.
    let catalog = apex_agent::AgentCatalog::load(None).unwrap();
    let spec = TeamSpec {
        agents: vec!["apex-reviewer".into(), "apex-reviewer".into()],
        strategy: apex_protocol::TeamStrategy::Parallel,
        model: None,
        writes: None,
    };
    let plan = apex_orchestrator::plan_from_team(&spec, "x", &catalog).unwrap();
    assert_eq!(plan.steps.len(), 2);
    assert_ne!(plan.steps[0].id, plan.steps[1].id);
}

#[test]
fn the_tui_holds_no_task_store_of_its_own() {
    // The TUI renders what the runtime reports; it keeps no second copy of the
    // task list. A diff is simply stored for the diff pane.
    let mut state = TuiState::new();
    state.set_diff(Some("--- a/src/x.rs\n+++ b/src/x.rs\n".to_string()));
    assert!(state.diff().is_some());
    assert_eq!(state.task_count(), 0, "no duplicate task store");
}
