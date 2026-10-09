//! The persistent APEX runtime: session manager, task lifecycle and dispatcher.

pub mod client;
pub mod ipc;
pub mod task;
pub mod transport;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use apex_agent::AgentCatalog;
use apex_core::config::Config;
use apex_core::error::{ApexError, Result};
use apex_core::{git, paths};
use apex_memory::{new_task, Store};
use apex_protocol::{
    AgentSummary, ChatMessage, Event, EventSink, ExecutionMode, RuntimeStatus, Task, TaskStatus,
};
use apex_tools::ToolRegistry;
use apex_verification::{verify, OutcomeContract};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub use task::{ApprovalBoard, TaskHandle, TaskSink};
pub use transport::ConnectionInfo;

/// Runtime version reported to clients.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

struct Inner {
    tasks: HashMap<String, TaskHandle>,
    broadcasts: HashMap<String, broadcast::Sender<Event>>,
}

/// The persistent runtime shared by all clients.
pub struct Runtime {
    pub config: Config,
    pub store: Arc<Store>,
    pub catalog: Arc<AgentCatalog>,
    pub registry: Arc<ToolRegistry>,
    pub approvals: Arc<ApprovalBoard>,
    pub info: Mutex<Option<ConnectionInfo>>,
    started_at: Instant,
    inner: Mutex<Inner>,
    last_activity: AtomicU64,
    shutdown: CancellationToken,
}

impl Runtime {
    /// Construct a runtime from configuration and a store.
    pub fn new(config: Config, store: Arc<Store>) -> Result<Arc<Runtime>> {
        let catalog = AgentCatalog::load(apex_core::paths::agents_dir().ok().as_deref())?;
        let runtime = Arc::new(Runtime {
            config,
            store,
            catalog: Arc::new(catalog),
            registry: Arc::new(ToolRegistry::default_set()),
            approvals: Arc::new(ApprovalBoard::default()),
            info: Mutex::new(None),
            started_at: Instant::now(),
            inner: Mutex::new(Inner {
                tasks: HashMap::new(),
                broadcasts: HashMap::new(),
            }),
            last_activity: AtomicU64::new(unix_now()),
            shutdown: CancellationToken::new(),
        });
        runtime.recover_interrupted_tasks()?;
        Ok(runtime)
    }

    /// Mark tasks left running by a previous process as interrupted.
    fn recover_interrupted_tasks(&self) -> Result<()> {
        for task in self.store.list_tasks(1000)? {
            if task.status == TaskStatus::Running {
                let mut task = task;
                task.status = TaskStatus::Failed;
                task.error = Some(
                    "task was interrupted by a runtime restart; use `apex task resume` to continue"
                        .into(),
                );
                task.updated_at = apex_core::now_rfc3339();
                task.finished_at = Some(task.updated_at.clone());
                self.store.update_task(&task)?;
                let event = self.store.append_event(
                    &task.id,
                    apex_protocol::EventKind::Error {
                        message: "runtime restarted while this task was running".into(),
                    },
                )?;
                let _ = event;
            }
        }
        Ok(())
    }

    /// Record client activity (used for idle shutdown).
    pub fn touch(&self) {
        self.last_activity.store(unix_now(), Ordering::Relaxed);
    }

    /// True when the runtime has been asked to shut down.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Whether the runtime should exit due to inactivity.
    pub fn should_exit_idle(&self) -> bool {
        let idle = self.config.runtime.idle_shutdown_secs;
        if idle == 0 {
            return false;
        }
        unix_now().saturating_sub(self.last_activity.load(Ordering::Relaxed)) > idle
    }

    /// Get or create a broadcast channel for a task.
    pub fn broadcast_channel(
        &self,
        task_id: &str,
    ) -> (broadcast::Sender<Event>, broadcast::Receiver<Event>) {
        let mut inner = self.inner.lock().unwrap();
        let tx = inner
            .broadcasts
            .entry(task_id.to_string())
            .or_insert_with(|| broadcast::channel(4096).0)
            .clone();
        let rx = tx.subscribe();
        (tx, rx)
    }

    /// Subscribe to live events for a task, returning the current last sequence.
    pub fn subscribe(&self, task_id: &str) -> Result<(broadcast::Receiver<Event>, i64)> {
        let (_tx, rx) = self.broadcast_channel(task_id);
        let last_seq = self.store.last_seq(task_id)?;
        Ok((rx, last_seq))
    }

    /// Get the cancellation token for a task, if running.
    pub fn cancel_token(&self, task_id: &str) -> CancellationToken {
        let inner = self.inner.lock().unwrap();
        inner
            .tasks
            .get(task_id)
            .map(|h| h.cancel.clone())
            .unwrap_or_else(CancellationToken::new)
    }

    /// Whether a task is currently running in this runtime.
    pub fn is_running(&self, task_id: &str) -> bool {
        self.inner.lock().unwrap().tasks.contains_key(task_id)
    }

    fn finish_task(&self, task_id: &str) {
        self.inner.lock().unwrap().tasks.remove(task_id);
    }

    fn finish_task_public(&self, task_id: &str) {
        self.finish_task(task_id);
    }

    /// Create and start a new task.
    pub fn create_task(
        self: &Arc<Self>,
        objective: String,
        project_root: String,
        model: Option<String>,
        agent_id: Option<String>,
        mode: ExecutionMode,
    ) -> Result<Task> {
        if mode != ExecutionMode::Single {
            return Err(ApexError::config(
                "only single-agent mode is implemented in this milestone; orchestrated and workflow modes are planned",
            ));
        }
        let root = PathBuf::from(&project_root);
        if !root.exists() {
            return Err(ApexError::Project(format!(
                "project root does not exist: {project_root}"
            )));
        }
        let task = new_task(objective, project_root, model, agent_id);
        let mut task = task;
        task.budget = self.config.budget.clone();
        task.mode = mode;
        self.store.create_task(&task)?;

        // Kick off execution.
        self.spawn_task(task.id.clone(), Vec::new(), None);

        // Emit TaskCreated event so subscribers see the start.
        let (tx, _rx) = self.broadcast_channel(&task.id);
        let sink = TaskSink::new(
            self.store.clone(),
            task.id.clone(),
            tx,
            self.config.memory.max_events_per_task as usize,
        );
        sink.emit(apex_protocol::EventKind::TaskCreated {
            objective: task.objective.clone(),
            project_root: task.project_root.clone(),
        });

        Ok(task)
    }

    fn spawn_task(
        self: &Arc<Self>,
        task_id: String,
        prior: Vec<ChatMessage>,
        extra: Option<String>,
    ) {
        let runtime = Arc::clone(self);
        let cancel = CancellationToken::new();
        let id = task_id.clone();
        let handle = tokio::spawn(async move {
            task::execute_task(runtime, id, prior, extra).await;
        });
        self.inner.lock().unwrap().tasks.insert(
            task_id.clone(),
            TaskHandle {
                cancel,
                run: handle,
            },
        );
    }

    /// Append an instruction. Only terminal tasks can be continued.
    pub async fn send_instruction(
        self: &Arc<Self>,
        task_id: &str,
        instruction: String,
    ) -> Result<()> {
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| ApexError::Storage(format!("task {task_id} not found")))?;
        if !task.status.is_terminal() {
            return Err(ApexError::config(
                "task is still running; cancel it or wait for completion before sending a new instruction",
            ));
        }
        let prior = task::prior_messages(&self.store, task_id)?;
        self.spawn_task(task_id.to_string(), prior, Some(instruction));
        Ok(())
    }

    /// Resume a terminal task using its persisted conversation.
    pub async fn resume_task(self: &Arc<Self>, task_id: &str) -> Result<()> {
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| ApexError::Storage(format!("task {task_id} not found")))?;
        if !task.status.is_terminal() {
            return Err(ApexError::config("task is not in a terminal state"));
        }
        if task.status == TaskStatus::Completed {
            return Err(ApexError::config(
                "task already completed; use `send instruction` to continue the conversation",
            ));
        }
        let prior = task::prior_messages(&self.store, task_id)?;
        self.spawn_task(task_id.to_string(), prior, None);
        Ok(())
    }

    /// Cancel a running task.
    pub fn cancel_task(&self, task_id: &str) -> Result<()> {
        let token = self.cancel_token(task_id);
        if self.is_running(task_id) {
            token.cancel();
            Ok(())
        } else {
            Err(ApexError::config("task is not running"))
        }
    }

    /// Agents available in this runtime.
    pub fn agents(&self) -> Vec<AgentSummary> {
        self.catalog
            .all()
            .iter()
            .map(|a| AgentSummary {
                id: a.id.clone(),
                name: a.name.clone(),
                version: a.version.clone(),
                description: a.description.clone(),
            })
            .collect()
    }

    /// Fetch the current diff for a task's project.
    pub async fn diff_for(&self, task_id: &str) -> Result<String> {
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| ApexError::Storage(format!("task {task_id} not found")))?;
        git::full_diff(&PathBuf::from(task.project_root)).await
    }

    /// Run verification now for a task's project.
    pub async fn verify_for(
        &self,
        task_id: &str,
    ) -> Result<(bool, Vec<apex_protocol::CheckResult>)> {
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| ApexError::Storage(format!("task {task_id} not found")))?;
        let root = PathBuf::from(&task.project_root);
        let contract = OutcomeContract::derive(&task.objective, &root);
        let report = verify(&root, &contract).await?;
        Ok((report.passed, report.checks))
    }

    /// Runtime status snapshot.
    pub fn status(&self) -> Result<RuntimeStatus> {
        Ok(RuntimeStatus {
            runtime_version: RUNTIME_VERSION.to_string(),
            protocol_version: apex_protocol::PROTOCOL_VERSION,
            uptime_secs: self.started_at.elapsed().as_secs(),
            task_count: self.store.count_tasks()?,
            active_task_count: self.inner.lock().unwrap().tasks.len(),
            database_path: paths::database_path()?.to_string_lossy().into_owned(),
        })
    }

    /// Resolve a pending approval.
    pub fn resolve_approval(&self, approval_id: &str, approved: bool) -> Result<bool> {
        Ok(self.approvals.resolve(approval_id, approved))
    }

    /// Trigger graceful shutdown: cancel tasks and remove the connection info.
    pub fn shutdown(&self) {
        let ids: Vec<String> = self.inner.lock().unwrap().tasks.keys().cloned().collect();
        for id in &ids {
            self.finish_task_public(id);
        }
        for id in ids {
            let _ = self.cancel_task(&id);
        }
        self.shutdown.cancel();
        if let Ok(path) = transport::runtime_info_path() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
