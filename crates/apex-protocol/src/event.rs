//! Structured, persisted events emitted during task execution.

use serde::{Deserialize, Serialize};

use crate::message::{ChatMessage, Usage};
use crate::task::TaskStatus;

/// Result of one verification check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckResult {
    /// Short name, e.g. "cargo build" or "git diff".
    pub name: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Human-readable detail (command output summary, reason, etc.).
    pub detail: String,
    /// Whether this check is advisory rather than required.
    #[serde(default)]
    pub advisory: bool,
}

/// The kind of a task event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    TaskCreated {
        objective: String,
        project_root: String,
    },
    StatusChanged {
        status: TaskStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Message {
        message: ChatMessage,
    },
    Plan {
        steps: Vec<String>,
    },
    ToolStarted {
        call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolFinished {
        call_id: String,
        name: String,
        success: bool,
        summary: String,
    },
    ApprovalRequested {
        approval_id: String,
        action: String,
        risk: String,
        detail: String,
    },
    ApprovalResolved {
        approval_id: String,
        approved: bool,
    },
    Usage {
        usage: Usage,
    },
    Verification {
        passed: bool,
        checks: Vec<CheckResult>,
    },
    Diff {
        summary: String,
    },
    /// A subtask (delegated agent) has started.
    SubtaskStarted {
        subtask_id: String,
        agent_id: String,
        objective: String,
        wave: usize,
    },
    /// A subtask has finished.
    SubtaskFinished {
        subtask_id: String,
        agent_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A computed execution plan, with the scheduling waves.
    PlanScheduled {
        /// Human-readable plan steps.
        steps: Vec<String>,
        /// How many waves the scheduler produced.
        waves: usize,
    },
    Log {
        level: String,
        message: String,
    },
    Error {
        message: String,
    },
}

impl EventKind {
    /// Short type tag used for display and filtering.
    pub fn tag(&self) -> &'static str {
        match self {
            EventKind::TaskCreated { .. } => "task_created",
            EventKind::StatusChanged { .. } => "status_changed",
            EventKind::Message { .. } => "message",
            EventKind::Plan { .. } => "plan",
            EventKind::ToolStarted { .. } => "tool_started",
            EventKind::ToolFinished { .. } => "tool_finished",
            EventKind::ApprovalRequested { .. } => "approval_requested",
            EventKind::ApprovalResolved { .. } => "approval_resolved",
            EventKind::Usage { .. } => "usage",
            EventKind::Verification { .. } => "verification",
            EventKind::Diff { .. } => "diff",
            EventKind::SubtaskStarted { .. } => "subtask_started",
            EventKind::SubtaskFinished { .. } => "subtask_finished",
            EventKind::PlanScheduled { .. } => "plan_scheduled",
            EventKind::Log { .. } => "log",
            EventKind::Error { .. } => "error",
        }
    }
}

/// A persisted event with a per-task monotonic sequence number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub task_id: String,
    /// Monotonic sequence number within the task (1-based).
    pub seq: i64,
    pub timestamp: String,
    #[serde(flatten)]
    pub kind: EventKind,
}
