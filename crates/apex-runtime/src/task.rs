//! Task execution: sinks, approvals, and the agent + verification + repair loop.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apex_agent::{AgentCatalog, AgentRunner, ApprovalRequest, Approver, RunInput};
use apex_core::config::Config;
use apex_core::error::{ApexError, Result};
use apex_memory::{MemoryScope, Store};
use apex_models::{FakeProvider, ModelProvider};
use apex_protocol::{ChatMessage, Event, EventKind, EventSink, Task, TaskStatus, Usage};
use apex_verification::{repair_instruction, verify, OutcomeContract};
use async_trait::async_trait;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;

use crate::Runtime;

/// Board for tracking pending human approvals.
#[derive(Default)]
pub struct ApprovalBoard {
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl ApprovalBoard {
    /// Register a pending approval so a client can resolve it later.
    pub fn register(&self, id: String, tx: oneshot::Sender<bool>) {
        self.pending.lock().unwrap().insert(id, tx);
    }

    /// Resolve a pending approval. Returns false if not found.
    pub fn resolve(&self, id: &str, approved: bool) -> bool {
        let sender = self.pending.lock().unwrap().remove(id);
        match sender {
            Some(tx) => tx.send(approved).is_ok(),
            None => false,
        }
    }

    /// Number of pending approvals.
    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

/// Approver that waits for a client to resolve, with a timeout (deny).
pub struct BoardApprover {
    board: Arc<ApprovalBoard>,
    timeout: Duration,
}

impl BoardApprover {
    pub fn new(board: Arc<ApprovalBoard>) -> Self {
        BoardApprover {
            board,
            timeout: Duration::from_secs(600),
        }
    }
}

#[async_trait]
impl Approver for BoardApprover {
    async fn approve(&self, request: &ApprovalRequest) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        self.board.register(request.approval_id.clone(), tx);
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(approved)) => Ok(approved),
            _ => Ok(false),
        }
    }
}

/// Emits events to both the store (persistence) and a broadcast channel (live).
pub struct TaskSink {
    store: Arc<Store>,
    task_id: String,
    tx: broadcast::Sender<Event>,
    max_events: usize,
}

impl TaskSink {
    pub fn new(
        store: Arc<Store>,
        task_id: String,
        tx: broadcast::Sender<Event>,
        max_events: usize,
    ) -> TaskSink {
        TaskSink {
            store,
            task_id,
            tx,
            max_events,
        }
    }
}

impl EventSink for TaskSink {
    fn emit(&self, kind: EventKind) {
        match self.store.append_event(&self.task_id, kind) {
            Ok(event) => {
                if self.store.event_count(&self.task_id).unwrap_or(0) % 100 == 0 {
                    let _ = self.store.trim_events(&self.task_id, self.max_events);
                }
                let _ = self.tx.send(event);
            }
            Err(e) => tracing::error!(error = %e, "failed to append event"),
        }
    }
}

/// A live handle to a running task.
pub struct TaskHandle {
    pub cancel: CancellationToken,
    pub run: tokio::task::JoinHandle<()>,
}

/// Resolve the provider/model to use, with an offline fallback.
pub fn resolve_runtime_model(config: &Config, requested: Option<&str>) -> (String, String) {
    let parse = |reference: &str| -> Option<(String, String)> {
        reference
            .split_once('/')
            .map(|(p, m)| (p.to_string(), m.to_string()))
    };
    if let Some(r) = requested {
        if let Some(parsed) = parse(r) {
            return parsed;
        }
    }
    if let Some(r) = &config.default_model {
        if let Some(parsed) = parse(r) {
            return parsed;
        }
    }
    for (name, provider) in &config.providers {
        if let Some(m) = &provider.default_model {
            return (name.clone(), m.clone());
        }
        if let Some(m) = provider.models.first() {
            return (name.clone(), m.clone());
        }
    }
    ("fake".into(), "fake-model".into())
}

/// Build the provider adapter for a resolved reference.
pub fn build_provider_for(config: &Config, provider_name: &str) -> Arc<dyn ModelProvider> {
    match apex_models::build_provider(config, provider_name) {
        Ok(provider) => provider,
        Err(e) => {
            tracing::warn!(provider = %provider_name, error = %e, "falling back to offline fake provider");
            Arc::new(FakeProvider::with_default_text(
                "Offline: no usable model provider is configured. Configure one with `apex models set`.",
            ))
        }
    }
}

/// Extract the conversation so far from persisted events.
pub fn prior_messages(store: &Store, task_id: &str) -> Result<Vec<ChatMessage>> {
    let events = store.events_after(task_id, 0)?;
    let mut messages = Vec::new();
    for event in events {
        if let EventKind::Message { message } = event.kind {
            messages.push(message);
        }
    }
    Ok(messages)
}

/// Execute a task end to end: agent loop, verification, bounded repair.
pub async fn execute_task(
    runtime: Arc<Runtime>,
    task_id: String,
    prior_messages: Vec<ChatMessage>,
    extra_instruction: Option<String>,
) {
    let store = runtime.store.clone();
    let result = execute_inner(
        runtime.clone(),
        task_id.clone(),
        prior_messages,
        extra_instruction,
    )
    .await;
    if let Err(e) = result {
        let message = format!("{e}");
        tracing::error!(task_id = %task_id, error = %message, "task execution failed");
        let mut task = match store.get_task(&task_id) {
            Ok(Some(t)) => t,
            _ => {
                runtime.finish_task(&task_id);
                return;
            }
        };
        let status = if matches!(e, ApexError::Cancelled) {
            TaskStatus::Cancelled
        } else {
            TaskStatus::Failed
        };
        task.status = status;
        task.error = Some(message.clone());
        task.updated_at = apex_core::now_rfc3339();
        if task.finished_at.is_none() {
            task.finished_at = Some(task.updated_at.clone());
        }
        let _ = store.update_task(&task);
        let (tx, _) = runtime.broadcast_channel(&task_id);
        let sink = TaskSink::new(
            store.clone(),
            task_id.clone(),
            tx,
            runtime.config.memory.max_events_per_task as usize,
        );
        sink.emit(EventKind::Error {
            message: message.clone(),
        });
        sink.emit(EventKind::StatusChanged {
            status,
            reason: Some(message),
        });
    }
    runtime.finish_task(&task_id);
}

async fn execute_inner(
    runtime: Arc<Runtime>,
    task_id: String,
    prior_messages: Vec<ChatMessage>,
    extra_instruction: Option<String>,
) -> Result<()> {
    let store = runtime.store.clone();
    let mut task = store
        .get_task(&task_id)?
        .ok_or_else(|| ApexError::Storage(format!("task {task_id} not found")))?;

    let workspace_root = std::path::PathBuf::from(&task.project_root);
    let agent = resolve_agent(&runtime.catalog, task.agent_id.as_deref());
    let (provider_name, model_id) = resolve_runtime_model(&runtime.config, task.model.as_deref());
    let provider = build_provider_for(&runtime.config, &provider_name);

    let runner = AgentRunner::new(provider, runtime.registry.clone());
    let approver = BoardApprover::new(runtime.approvals.clone());

    // Mark running.
    task.status = TaskStatus::Running;
    task.started_at.get_or_insert_with(apex_core::now_rfc3339);
    task.updated_at = apex_core::now_rfc3339();
    task.error = None;
    task.summary = None;
    store.update_task(&task)?;

    let (tx, _rx) = runtime.broadcast_channel(&task_id);
    let sink = TaskSink::new(
        store.clone(),
        task_id.clone(),
        tx,
        runtime.config.memory.max_events_per_task as usize,
    );
    sink.emit(EventKind::StatusChanged {
        status: TaskStatus::Running,
        reason: Some(format!(
            "agent {} using {provider_name}/{model_id}",
            agent.id
        )),
    });

    let cancel = runtime.cancel_token(&task_id);
    let context_notes = load_context(&store, &task, &agent.memory.scope, &agent.id);

    let objective = extra_instruction.unwrap_or_else(|| task.objective.clone());

    let mut input = RunInput {
        task_id: task_id.clone(),
        objective: objective.clone(),
        workspace_root: workspace_root.clone(),
        permissions: runtime.config.permissions.clone(),
        budget: task.budget.clone(),
        model: model_id.clone(),
        cancel: cancel.clone(),
        prior_messages: prior_messages.clone(),
        context_notes,
    };

    let mut usage = Usage::default();
    let mut tool_calls = 0u32;
    let mut steps = 0u32;
    let mut summary;
    let mut failed: Option<String> = None;

    let outcome = runner.run(&agent, input.clone(), &sink, &approver).await?;
    usage.accumulate(&outcome.usage);
    tool_calls += outcome.tool_calls;
    steps += outcome.steps;
    summary = outcome.summary.clone();
    if outcome.status == TaskStatus::Failed {
        failed = outcome.error.clone();
    }

    // Verification + bounded repair.
    let contract = OutcomeContract::derive(&task.objective, &workspace_root);
    let mut report = verify(&workspace_root, &contract).await?;
    let mut attempt = 0u32;

    while !report.passed && failed.is_none() && attempt < task.budget.max_repair_attempts {
        if cancel.is_cancelled() {
            return Err(ApexError::Cancelled);
        }
        attempt += 1;
        sink.emit(EventKind::StatusChanged {
            status: TaskStatus::Verifying,
            reason: Some(format!("repair attempt {attempt}")),
        });
        let repair = repair_instruction(&report, attempt);
        input = RunInput {
            task_id: task_id.clone(),
            objective: repair,
            workspace_root: workspace_root.clone(),
            permissions: runtime.config.permissions.clone(),
            budget: task.budget.clone(),
            model: model_id.clone(),
            cancel: cancel.clone(),
            prior_messages: outcome.messages.clone(),
            context_notes: Vec::new(),
        };
        let repair_outcome = runner.run(&agent, input.clone(), &sink, &approver).await?;
        usage.accumulate(&repair_outcome.usage);
        tool_calls += repair_outcome.tool_calls;
        steps += repair_outcome.steps;
        summary = repair_outcome.summary.clone();
        report = verify(&workspace_root, &contract).await?;
    }

    sink.emit(EventKind::Verification {
        passed: report.passed,
        checks: report.checks.clone(),
    });
    if !report.diff_summary.trim().is_empty() {
        sink.emit(EventKind::Diff {
            summary: report.diff_summary.clone(),
        });
    }

    let status = if failed.is_some() {
        TaskStatus::Failed
    } else if report.passed {
        TaskStatus::Completed
    } else {
        TaskStatus::Failed
    };

    let error = failed.or_else(|| {
        if report.passed {
            None
        } else {
            let failures: Vec<String> = report.failures().iter().map(|c| c.name.clone()).collect();
            Some(format!(
                "verification failed after {} repair attempt(s): {}",
                attempt,
                failures.join(", ")
            ))
        }
    });

    // Persist a lesson when verification succeeded but repairs were needed.
    if report.passed && attempt > 0 && agent.memory.persist_lessons {
        let lesson = format!(
            "Task '{}' needed {attempt} repair attempt(s) to pass verification.",
            task.objective
        );
        let _ = store.add_note(
            MemoryScope::Project,
            Some(&task.project_root),
            Some(&agent.id),
            "verification",
            &lesson,
        );
    }

    task.status = status;
    task.usage = usage;
    task.tool_calls = tool_calls;
    task.steps = steps;
    task.repair_attempts = attempt;
    task.summary = Some(summary);
    task.error = error;
    task.updated_at = apex_core::now_rfc3339();
    task.finished_at = Some(task.updated_at.clone());
    store.update_task(&task)?;

    sink.emit(EventKind::StatusChanged {
        status,
        reason: task.error.clone(),
    });

    if status == TaskStatus::Failed {
        if let Some(message) = task.error.clone() {
            sink.emit(EventKind::Error { message });
        }
    }

    Ok(())
}

fn resolve_agent<'a>(
    catalog: &'a AgentCatalog,
    agent_id: Option<&str>,
) -> &'a apex_agent::AgentManifest {
    match agent_id.and_then(|id| catalog.get(id)) {
        Some(agent) => agent,
        None => catalog.default_agent(),
    }
}

fn load_context(store: &Store, task: &Task, scope: &str, agent_id: &str) -> Vec<String> {
    let (memory_scope, project, agent) = match scope {
        "agent" => (
            MemoryScope::Agent,
            Some(task.project_root.as_str()),
            Some(agent_id),
        ),
        "user" => (MemoryScope::User, None, None),
        _ => (MemoryScope::Project, Some(task.project_root.as_str()), None),
    };
    match store.notes(memory_scope, project, agent, 20) {
        Ok(notes) => notes
            .into_iter()
            .map(|n| format!("[{}] {}", n.kind, n.content))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Convenience for tests: wait for a task to reach a terminal status.
pub async fn wait_for_terminal(store: &Store, task_id: &str, timeout: Duration) -> Result<Task> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(task) = store.get_task(task_id)? {
            if task.status.is_terminal() {
                return Ok(task);
            }
        }
        if Instant::now() > deadline {
            return Err(ApexError::Protocol("timeout waiting for task".into()));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
