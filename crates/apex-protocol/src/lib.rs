//! APEX wire protocol and shared domain model.

pub mod event;
pub mod message;
pub mod plan;
pub mod sink;
pub mod subtask;
pub mod task;
pub mod team;
pub mod wire;
pub mod workflow;

pub use event::{CheckResult, Event, EventKind};
pub use message::{ChatMessage, Role, ToolCall, ToolSpec, Usage};
pub use plan::{Plan, PlannedStep, MATCH_ALL};
pub use sink::{EventSink, NullSink};
pub use subtask::{Subtask, SubtaskLine, SubtaskStatus};
pub use task::{ExecutionMode, Task, TaskStatus};
pub use team::{PlannedTask, TeamSpec, TeamStrategy};
pub use wire::{AgentSummary, Frame, Request, Response, RuntimeStatus, PROTOCOL_VERSION};
pub use workflow::{WorkflowDefinition, WorkflowStep};

/// Crate version reported during the handshake.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");
