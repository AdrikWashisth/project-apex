//! Task domain model.

use apex_core::config::Budget;
use serde::{Deserialize, Serialize};

use crate::message::Usage;

/// Lifecycle status of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    WaitingApproval,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::WaitingApproval => "waiting_approval",
            TaskStatus::Verifying => "verifying",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

/// How a task is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// A single agent performs the task end to end.
    #[default]
    Single,
    /// The user assigns work to selected agents.
    ManualMulti,
    /// An orchestrator plans, delegates and verifies.
    Orchestrated,
    /// Agents execute a user-defined sequence or graph.
    Workflow,
}

/// A persisted task record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub objective: String,
    pub project_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub mode: ExecutionMode,
    pub status: TaskStatus,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub budget: Budget,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub tool_calls: u32,
    #[serde(default)]
    pub steps: u32,
    #[serde(default)]
    pub repair_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
