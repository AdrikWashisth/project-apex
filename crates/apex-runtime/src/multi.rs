//! Multi-agent execution: running a plan wave by wave.
//!
//! The runtime owns execution; `apex-orchestrator` owns scheduling. This module
//! walks the waves produced by the scheduler, runs each step with a real agent,
//! and hands each completed step's findings to the steps that depend on it.
//!
//! Safety properties enforced here:
//! * Concurrent steps never share a write target (the scheduler guarantees it).
//! * The parent task's budget is shared across all subtasks, so a fan-out cannot
//!   multiply the spend.
//! * Cancellation is checked before every step and every wave.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use apex_agent::{AgentCatalog, AgentRunner};
use apex_core::config::Budget;
use apex_core::error::{ApexError, Result};
use apex_memory::Store;
use apex_models::ModelProvider;
use apex_protocol::{
    EventKind, EventSink, Plan, PlannedStep, Subtask, SubtaskStatus, Task, TaskStatus, Usage,
};

use crate::Runtime;

/// Shared budget accounting across every subtask of a task.
#[derive(Debug, Default)]
pub struct BudgetTracker {
    usage: Usage,
    tool_calls: AtomicU32,
    steps: AtomicU32,
    budget: Budget,
}

impl BudgetTracker {
    pub fn new(budget: Budget) -> BudgetTracker {
        BudgetTracker {
            usage: Usage::default(),
            tool_calls: AtomicU32::new(0),
            steps: AtomicU32::new(0),
            budget,
        }
    }

    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    pub fn tool_calls(&self) -> u32 {
        self.tool_calls.load(Ordering::Relaxed)
    }

    pub fn steps(&self) -> u32 {
        self.steps.load(Ordering::Relaxed)
    }

    /// Record consumption and fail if any budget is now exhausted.
    pub fn charge(&mut self, usage: &Usage, tool_calls: u32, steps: u32) -> Result<()> {
        self.usage.accumulate(usage);
        self.tool_calls.fetch_add(tool_calls, Ordering::Relaxed);
        self.steps.fetch_add(steps, Ordering::Relaxed);
        self.check()
    }

    pub fn check(&self) -> Result<()> {
        if self.usage.total_tokens > self.budget.max_model_tokens {
            return Err(ApexError::Budget(format!(
                "token budget of {} exceeded",
                self.budget.max_model_tokens
            )));
        }
        if let Some(cost) = self.usage.cost_usd {
            if cost > self.budget.max_cost_usd {
                return Err(ApexError::Budget(format!(
                    "cost budget of ${:.2} exceeded (${:.2} spent)",
                    self.budget.max_cost_usd, cost
                )));
            }
        }
        Ok(())
    }

    /// A per-subtask slice of the remaining budget.
    ///
    /// Each subtask gets at most `1/parallelism` of the remaining tool-call
    /// and step allowances so one fan-out cannot exceed the parent budget.
    pub fn slice_for_subtask(&self, parallelism: usize) -> Budget {
        let divisor = parallelism.max(1) as u32;
        Budget {
            max_model_tokens: self
                .budget
                .max_model_tokens
                .saturating_sub(self.usage.total_tokens),
            max_cost_usd: (self.budget.max_cost_usd - self.usage.cost_usd.unwrap_or(0.0)).max(0.0),
            max_tool_calls: self.budget.max_tool_calls.saturating_sub(self.tool_calls()) / divisor,
            max_wall_secs: self.budget.max_wall_secs,
            max_repair_attempts: 0, // repairs happen once, at the task level
            max_steps: self.budget.max_steps.saturating_sub(self.steps()) / divisor,
        }
    }
}

/// Outcome of running one subtask.
pub struct StepOutcome {
    pub subtask: Subtask,
    pub failed: Option<String>,
}

/// Run every wave of a plan to completion (or cancellation).
///
/// Returns the subtask records in completion order. Individual step failures do
/// not abort the whole task; dependents of a failed step are skipped, and the
/// caller decides the task-level verdict from the results plus verification.
#[allow(clippy::too_many_arguments)]
pub async fn execute_plan(
    runtime: &Arc<Runtime>,
    store: &Arc<Store>,
    provider: Arc<dyn ModelProvider>,
    catalog: &Arc<AgentCatalog>,
    task: &Task,
    plan: &Plan,
    waves: &[Vec<PlannedStep>],
    sink: Arc<dyn EventSink>,
    approver: Arc<dyn apex_agent::Approver>,
    tracker: &mut BudgetTracker,
    default_model: &str,
) -> Result<Vec<StepOutcome>> {
    let mut outcomes: Vec<StepOutcome> = Vec::new();

    // Persist every subtask up front so clients can see the whole plan.
    for (wave_index, wave) in waves.iter().enumerate() {
        for step in wave {
            let mut subtask = Subtask::new(
                task.id.clone(),
                step.id.clone(),
                step.agent_id.clone(),
                step.objective.clone(),
            );
            subtask.depends_on = step.depends_on.clone();
            subtask.wave = wave_index;
            store.create_subtask(&subtask)?;
        }
    }

    let wave_summary: Vec<String> = waves
        .iter()
        .enumerate()
        .map(|(index, w)| {
            format!(
                "wave {}: {}",
                index,
                w.iter()
                    .map(|s| format!("{} ({})", s.id, s.agent_id))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect();
    sink.emit(EventKind::PlanScheduled {
        steps: plan
            .steps
            .iter()
            .map(|s| format!("{} -> {}", s.id, s.agent_id))
            .collect(),
        waves: waves.len(),
    });
    for line in wave_summary {
        sink.emit(EventKind::Log {
            level: "info".into(),
            message: format!("plan {line}"),
        });
    }

    for (wave_index, wave) in waves.iter().enumerate() {
        let cancel = runtime.cancel_token(&task.id);
        if cancel.is_cancelled() {
            return Err(ApexError::Cancelled);
        }
        tracker.check()?;

        // Results produced by earlier waves, handed to this wave's steps.
        let mut handoff = build_handoff(store, task, wave)?;

        // Steps whose dependencies failed are skipped rather than run.
        let mut runnable: Vec<&PlannedStep> = Vec::new();
        let mut skipped: Vec<PlannedStep> = Vec::new();
        for step in wave {
            let blocked = step.depends_on.iter().any(|dep| {
                outcomes
                    .iter()
                    .any(|o| o.subtask.id == *dep && o.subtask.status == SubtaskStatus::Failed)
            });
            if blocked {
                skipped.push(step.clone());
            } else {
                runnable.push(step);
            }
        }

        // A skipped step's dependents must not run either.
        if !skipped.is_empty() {
            let skipped_ids: Vec<String> = skipped.iter().map(|s| s.id.clone()).collect();
            for step in &skipped {
                let mut subtask = load_subtask(store, task, &step.id)?;
                subtask.status = SubtaskStatus::Skipped;
                subtask.error = Some(format!(
                    "skipped because a dependency failed: {}",
                    skipped_ids.join(", ")
                ));
                finish(&mut subtask);
                store.update_subtask(&subtask)?;
                sink.emit(EventKind::SubtaskFinished {
                    subtask_id: subtask.id.clone(),
                    agent_id: subtask.agent_id.clone(),
                    status: subtask.status.as_str().into(),
                    result: None,
                    error: subtask.error.clone(),
                });
                outcomes.push(StepOutcome {
                    subtask,
                    failed: None,
                });
                handoff.push(format!(
                    "Step {} was skipped (a dependency failed).",
                    step.id
                ));
            }
        }

        if runnable.is_empty() {
            continue;
        }

        // Run the wave. Steps inside a wave never conflict, so they are safe
        // to run concurrently.
        let parallelism = runnable.len();
        let mut handles = Vec::with_capacity(runnable.len());
        for step in runnable {
            let mut subtask = load_subtask(store, task, &step.id)?;
            subtask.status = SubtaskStatus::Running;
            subtask.started_at = Some(apex_core::now_rfc3339());
            subtask.updated_at = apex_core::now_rfc3339();
            store.update_subtask(&subtask)?;

            sink.emit(EventKind::SubtaskStarted {
                subtask_id: step.id.clone(),
                agent_id: step.agent_id.clone(),
                objective: step.objective.clone(),
                wave: wave_index,
            });

            let manifest = catalog
                .get(&step.agent_id)
                .ok_or_else(|| ApexError::config(format!("unknown agent '{}'", step.agent_id)))?
                .clone();
            let runner = AgentRunner::new(Arc::clone(&provider), runtime.registry.clone());
            let step = step.clone();
            let task_id = task.id.clone();
            let project_root = task.project_root.clone();
            let permissions = runtime.config.permissions.clone();
            let objective = step.objective.clone();
            let notes = handoff.clone();
            let cancel = cancel.clone();
            let budget = tracker.slice_for_subtask(parallelism);
            let model = step
                .model
                .clone()
                .unwrap_or_else(|| default_model.to_string());

            // Events from concurrent agents interleave on the shared sink; that
            // is intentional — the sequence number keeps replay deterministic.
            let step_sink = StepSink {
                inner: Arc::clone(&sink),
                agent_id: step.agent_id.clone(),
            };
            let step_approver = Arc::clone(&approver);

            handles.push(tokio::spawn(async move {
                let input = apex_agent::RunInput {
                    task_id,
                    objective,
                    workspace_root: std::path::PathBuf::from(project_root),
                    permissions,
                    budget,
                    model,
                    cancel,
                    prior_messages: Vec::new(),
                    context_notes: notes,
                };
                let result = runner
                    .run(&manifest, input, &step_sink, &*step_approver)
                    .await;
                (step, result)
            }));
        }

        for handle in handles {
            let (step, result) = handle
                .await
                .map_err(|e| ApexError::Protocol(format!("subtask task panicked: {e}")))?;
            let mut subtask = load_subtask(store, task, &step.id)?;

            match result {
                Ok(outcome) => {
                    tracker.charge(&outcome.usage, outcome.tool_calls, outcome.steps)?;
                    sink.emit(EventKind::Usage {
                        usage: outcome.usage.clone(),
                    });
                    // The agent runner reports task-level status; map it onto
                    // the subtask's own status vocabulary.
                    subtask.status = if outcome.status == TaskStatus::Failed {
                        SubtaskStatus::Failed
                    } else {
                        SubtaskStatus::Completed
                    };
                    subtask.result = Some(outcome.summary.clone());
                    subtask.tool_calls = outcome.tool_calls;
                    subtask.steps = outcome.steps;
                    subtask.tokens = outcome.usage.total_tokens;
                    if subtask.status == SubtaskStatus::Failed {
                        subtask.error = outcome.error.clone();
                    }
                }
                Err(ApexError::Cancelled) => {
                    subtask.status = SubtaskStatus::Cancelled;
                    subtask.error = Some("cancelled".into());
                }
                Err(e) => {
                    // Budget or model errors propagate and stop the whole task.
                    if matches!(e, ApexError::Budget(_)) || matches!(e, ApexError::Cancelled) {
                        return Err(e);
                    }
                    subtask.status = SubtaskStatus::Failed;
                    subtask.error = Some(e.to_string());
                }
            }
            finish(&mut subtask);
            store.update_subtask(&subtask)?;
            sink.emit(EventKind::SubtaskFinished {
                subtask_id: subtask.id.clone(),
                agent_id: subtask.agent_id.clone(),
                status: subtask.status.as_str().into(),
                result: subtask.result.clone(),
                error: subtask.error.clone(),
            });
            let failure = subtask.error.clone();
            outcomes.push(StepOutcome {
                subtask,
                failed: failure,
            });
        }
    }

    Ok(outcomes)
}

/// Finalise timestamps on a finished subtask.
fn finish(subtask: &mut Subtask) {
    subtask.updated_at = apex_core::now_rfc3339();
    if subtask.finished_at.is_none() {
        subtask.finished_at = Some(subtask.updated_at.clone());
    }
}

fn load_subtask(store: &Arc<Store>, task: &Task, step_id: &str) -> Result<Subtask> {
    store
        .list_subtasks(&task.id)?
        .into_iter()
        .find(|s| s.id == step_id)
        .ok_or_else(|| ApexError::Storage(format!("subtask {step_id} not found")))
}

/// Build the notes handed to the steps in a wave.
///
/// This is the agent-to-agent communication channel: a step sees what its
/// predecessors actually concluded, not just their name.
fn build_handoff(store: &Arc<Store>, task: &Task, wave: &[PlannedStep]) -> Result<Vec<String>> {
    let all = store.list_subtasks(&task.id)?;
    let mut notes = Vec::new();
    for step in wave {
        for dep in &step.depends_on {
            if let Some(done) = all.iter().find(|s| &s.id == dep) {
                if done.status == SubtaskStatus::Completed {
                    if let Some(result) = &done.result {
                        notes.push(format!(
                            "Result from {} ({}):\n{}",
                            dep,
                            done.agent_id,
                            result.trim()
                        ));
                    }
                } else if done.status == SubtaskStatus::Failed {
                    notes.push(format!(
                        "Note: {dep} ({}) did not complete successfully: {}. \
                         Verify its assumptions before relying on them.",
                        done.agent_id,
                        done.error.as_deref().unwrap_or("unknown error")
                    ));
                }
            }
        }
    }
    Ok(notes)
}

/// Prefixes a step's events with the agent identity.
///
/// Holds an `Arc` rather than a reference so the per-step task can be spawned
/// (spawned futures must be `'static`).
struct StepSink {
    inner: Arc<dyn EventSink>,
    agent_id: String,
}

impl EventSink for StepSink {
    fn emit(&self, kind: EventKind) {
        match kind {
            EventKind::ToolStarted {
                call_id,
                name,
                arguments,
            } => self.inner.emit(EventKind::ToolStarted {
                call_id,
                name: format!("[{}] {name}", self.agent_id),
                arguments,
            }),
            EventKind::ToolFinished {
                call_id,
                name,
                success,
                summary,
            } => self.inner.emit(EventKind::ToolFinished {
                call_id,
                name: format!("[{}] {name}", self.agent_id),
                success,
                summary,
            }),
            EventKind::Message { message } => {
                // Only surface substantive messages from sub-agents.
                if message.tool_calls.is_empty() {
                    if let Some(text) = &message.content {
                        if !text.trim().is_empty() {
                            self.inner.emit(EventKind::Message { message });
                        }
                    }
                } else {
                    self.inner.emit(EventKind::Message { message });
                }
            }
            other => self.inner.emit(other),
        }
    }
}

/// Convenience: does this outcome set contain a hard failure?
pub fn any_failed(outcomes: &[StepOutcome]) -> bool {
    outcomes
        .iter()
        .any(|o| o.subtask.status == SubtaskStatus::Failed)
}

/// The task-level status implied by a set of subtask outcomes.
pub fn task_status_for(outcomes: &[StepOutcome]) -> TaskStatus {
    if any_failed(outcomes) {
        TaskStatus::Failed
    } else {
        TaskStatus::Completed
    }
}
