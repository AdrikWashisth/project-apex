//! Subtasks: the individually tracked units of a multi-agent task.

use serde::{Deserialize, Serialize};

/// Lifecycle status of a subtask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl SubtaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            SubtaskStatus::Completed
                | SubtaskStatus::Failed
                | SubtaskStatus::Skipped
                | SubtaskStatus::Cancelled
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SubtaskStatus::Pending => "pending",
            SubtaskStatus::Running => "running",
            SubtaskStatus::Completed => "completed",
            SubtaskStatus::Failed => "failed",
            SubtaskStatus::Skipped => "skipped",
            SubtaskStatus::Cancelled => "cancelled",
        }
    }
}

/// One delegated unit of work inside a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subtask {
    /// Stable id, unique within the task.
    pub id: String,
    /// The parent task.
    pub task_id: String,
    /// The agent performing this subtask.
    pub agent_id: String,
    /// What this subtask must accomplish.
    pub objective: String,
    /// Subtask ids that must complete first.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Scheduling wave this subtask belongs to.
    #[serde(default)]
    pub wave: usize,
    pub status: SubtaskStatus,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// The agent's final message, handed to dependent subtasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default)]
    pub tool_calls: u32,
    #[serde(default)]
    pub steps: u32,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Subtask {
    /// Create a pending subtask.
    pub fn new(
        task_id: impl Into<String>,
        id: impl Into<String>,
        agent_id: impl Into<String>,
        objective: impl Into<String>,
    ) -> Subtask {
        let now = apex_core::now_rfc3339();
        Subtask {
            id: id.into(),
            task_id: task_id.into(),
            agent_id: agent_id.into(),
            objective: objective.into(),
            depends_on: Vec::new(),
            wave: 0,
            status: SubtaskStatus::Pending,
            created_at: now.clone(),
            updated_at: now,
            started_at: None,
            finished_at: None,
            result: None,
            tool_calls: 0,
            steps: 0,
            tokens: 0,
            error: None,
        }
    }
}

/// A summarised plan for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtaskLine {
    pub id: String,
    pub agent_id: String,
    pub status: SubtaskStatus,
    pub wave: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}
