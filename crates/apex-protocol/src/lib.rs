//! APEX wire protocol and shared domain model.

pub mod event;
pub mod message;
pub mod sink;
pub mod task;
pub mod wire;

pub use event::{CheckResult, Event, EventKind};
pub use message::{ChatMessage, Role, ToolCall, ToolSpec, Usage};
pub use sink::{EventSink, NullSink};
pub use task::{ExecutionMode, Task, TaskStatus};
pub use wire::{AgentSummary, Frame, Request, Response, RuntimeStatus, PROTOCOL_VERSION};

/// Crate version reported during the handshake.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");
