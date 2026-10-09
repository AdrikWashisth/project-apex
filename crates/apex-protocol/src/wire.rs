//! Versioned request/response protocol shared by the CLI, IDE extension and
//! the persistent runtime. Frames are newline-delimited JSON.

use serde::{Deserialize, Serialize};

use crate::event::Event;
use crate::task::{ExecutionMode, Task};

/// Current protocol version. Bump on breaking changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// A client request to the runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Handshake. Must be the first frame from a client.
    Hello {
        protocol_version: u32,
        client_name: String,
        client_version: String,
        /// Bearer token from the published runtime connection info.
        token: String,
    },
    /// Create and start a new task.
    CreateTask {
        objective: String,
        project_root: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        agent_id: Option<String>,
        #[serde(default)]
        mode: ExecutionMode,
        /// Manual multi-agent team. Used when `mode` is `manual_multi`.
        #[serde(default)]
        team: Option<crate::team::TeamSpec>,
        /// Explicit execution plan. Used when `mode` is `orchestrated` or
        /// when the caller wants to supply a plan directly.
        #[serde(default)]
        plan: Option<crate::team::PlannedTask>,
        /// Workflow definition. Used when `mode` is `workflow`; the runtime
        /// converts it to a plan using its agent catalog.
        #[serde(default)]
        workflow: Option<crate::workflow::WorkflowDefinition>,
    },
    /// List recent tasks.
    ListTasks {
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Fetch a single task.
    ShowTask { task_id: String },
    /// List the subtasks of a task.
    ListSubtasks { task_id: String },
    /// Return the plan and scheduling waves for a task.
    ShowPlan { task_id: String },
    /// Append an instruction to a running or paused task.
    SendInstruction {
        task_id: String,
        instruction: String,
    },
    /// Fetch events after a given sequence number.
    TaskEvents {
        task_id: String,
        #[serde(default)]
        after_seq: Option<i64>,
    },
    /// List registered agents.
    ListAgents,
    /// Cancel a task.
    CancelTask { task_id: String },
    /// Resume a previously interrupted task.
    ResumeTask { task_id: String },
    /// Approve or reject a pending action.
    ResolveApproval {
        task_id: String,
        approval_id: String,
        approved: bool,
    },
    /// Retrieve the current Git diff for a task's project.
    GetDiff { task_id: String },
    /// Run verification for a task.
    Verify { task_id: String },
    /// Runtime status.
    Status,
    /// Ask the runtime to shut down gracefully.
    Shutdown,
}

/// A runtime response.
///
/// Task lists, event batches and diffs are boxed because they are far larger
/// than the other variants. This keeps `Response` — and therefore every frame
/// on the wire — small.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Response {
    Hello {
        protocol_version: u32,
        runtime_version: String,
        session_id: String,
    },
    Ok,
    Task {
        task: Box<Task>,
    },
    /// The subtasks of a multi-agent task.
    SubtaskList {
        task_id: String,
        subtasks: Box<Vec<crate::subtask::Subtask>>,
    },
    /// The execution plan and its scheduling waves.
    Plan {
        task_id: String,
        plan: Box<crate::plan::Plan>,
        /// Scheduling wave index for each step, in step order.
        waves: Box<Vec<usize>>,
    },
    TaskList {
        tasks: Box<Vec<Task>>,
    },
    Events {
        events: Box<Vec<Event>>,
        last_seq: i64,
    },
    Agents {
        agents: Vec<AgentSummary>,
    },
    Diff {
        diff: Box<String>,
    },
    Verification {
        passed: bool,
        checks: Vec<crate::event::CheckResult>,
    },
    Status {
        status: RuntimeStatus,
    },
    Error {
        code: String,
        message: String,
    },
}

/// A frame on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum Frame {
    /// A client request awaiting a response with the matching `id`.
    Request { id: u64, request: Request },
    /// A response to a prior request.
    Response { id: u64, response: Response },
    /// An unsolicited event pushed by the runtime.
    Event { event: Event },
}

/// Summary of a registered agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
}

/// Runtime status snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub runtime_version: String,
    pub protocol_version: u32,
    pub uptime_secs: u64,
    pub task_count: usize,
    pub active_task_count: usize,
    pub database_path: String,
}

impl Response {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Response::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}
