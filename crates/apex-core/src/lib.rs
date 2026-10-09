//! APEX core: shared configuration, project discovery, Git integration,
//! process execution and safety utilities.

pub mod config;
pub mod error;
pub mod git;
pub mod paths;
pub mod process;
pub mod project;

pub use config::{
    Budget, Config, PermissionProfile, Permissions, ProviderKind, RiskClass, Transport,
};
pub use error::{ApexError, Result};
pub use process::{CommandOutput, RunOptions};
pub use project::Project;

/// Helper to produce an RFC 3339 timestamp in UTC.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Generate a new random identifier with the given short prefix.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
