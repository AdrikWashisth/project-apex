//! Error types shared across APEX crates.

use std::path::PathBuf;

/// Convenient result alias used throughout APEX.
pub type Result<T> = std::result::Result<T, ApexError>;

/// Top-level error type for APEX.
#[derive(Debug, thiserror::Error)]
pub enum ApexError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("project error: {0}")]
    Project(String),

    #[error("git error: {0}")]
    Git(String),

    #[error("model provider error ({provider}): {message}")]
    Model { provider: String, message: String },

    #[error("tool error ({tool}): {message}")]
    Tool { tool: String, message: String },

    #[error("permission denied: {0}")]
    Permission(String),

    /// A path escaped the configured workspace boundary.
    #[error("path {path} is outside the allowed workspace {root}")]
    PathEscape { path: PathBuf, root: PathBuf },

    #[error("budget exceeded: {0}")]
    Budget(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("cancelled")]
    Cancelled,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl ApexError {
    pub fn config(msg: impl Into<String>) -> Self {
        ApexError::Config(msg.into())
    }

    pub fn model(provider: impl Into<String>, msg: impl Into<String>) -> Self {
        ApexError::Model {
            provider: provider.into(),
            message: msg.into(),
        }
    }

    pub fn tool(tool: impl Into<String>, msg: impl Into<String>) -> Self {
        ApexError::Tool {
            tool: tool.into(),
            message: msg.into(),
        }
    }
}
