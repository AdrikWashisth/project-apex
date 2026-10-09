//! Versioned agent manifest format (TOML).
//!
//! Manifests declare identity, behaviour, model preferences, tool access,
//! execution limits, memory policy and evaluation cases. They are validated
//! before use; a manifest is never treated as trusted executable code.

use serde::{Deserialize, Serialize};

use apex_core::error::{ApexError, Result};

/// Model preferences for an agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelPrefs {
    /// Preferred `provider/model` reference.
    pub preferred: Option<String>,
    /// Ordered fallback references.
    pub fallback: Vec<String>,
    /// Capabilities the model must support (e.g. "tools").
    pub requires: Vec<String>,
}

/// Tool access policy for an agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolPrefs {
    /// Allowed tool names. Empty means "all tools permitted by the profile".
    pub allowed: Vec<String>,
    /// Denied tool names (takes precedence over `allowed`).
    pub denied: Vec<String>,
}

/// Write scope declared by the agent itself.
///
/// A declaration lets two write-capable agents run concurrently when their
/// scopes are provably disjoint. An empty declaration means "may modify the
/// whole workspace", which is never safe to parallelise.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WritePrefs {
    /// Path globs this agent may modify. Empty means the whole workspace.
    pub paths: Vec<String>,
}

/// Execution limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecutionLimits {
    pub timeout_secs: u64,
    pub max_steps: u32,
    pub max_tool_calls: u32,
    pub max_tokens: u64,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        ExecutionLimits {
            timeout_secs: 1800,
            max_steps: 60,
            max_tool_calls: 200,
            max_tokens: 500_000,
        }
    }
}

/// Memory policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryPrefs {
    /// Scope: "project", "agent" or "user".
    pub scope: String,
    /// Whether to persist notes after a task completes.
    pub persist_lessons: bool,
}

impl Default for MemoryPrefs {
    fn default() -> Self {
        MemoryPrefs {
            scope: "project".into(),
            persist_lessons: true,
        }
    }
}

/// A complete agent manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub maintainer: String,
    /// System instructions defining the agent's role and behaviour.
    pub instructions: String,
    /// Task types this agent handles (free-form tags).
    pub task_types: Vec<String>,
    pub models: ModelPrefs,
    pub tools: ToolPrefs,
    /// Declared write scope, used for parallel scheduling.
    #[serde(default)]
    pub writes: WritePrefs,
    pub execution: ExecutionLimits,
    pub memory: MemoryPrefs,
    /// Evaluation case identifiers used to test the agent.
    pub evaluation: Vec<String>,
}

impl Default for AgentManifest {
    fn default() -> Self {
        AgentManifest {
            id: String::new(),
            name: String::new(),
            version: "0.1.0".into(),
            description: String::new(),
            maintainer: String::new(),
            instructions: String::new(),
            task_types: Vec::new(),
            models: ModelPrefs::default(),
            tools: ToolPrefs::default(),
            writes: WritePrefs::default(),
            execution: ExecutionLimits::default(),
            memory: MemoryPrefs::default(),
            evaluation: Vec::new(),
        }
    }
}

impl AgentManifest {
    /// Parse a manifest from TOML text.
    pub fn from_toml(text: &str) -> Result<AgentManifest> {
        let manifest: AgentManifest = toml::from_str(text)
            .map_err(|e| ApexError::config(format!("invalid agent manifest: {e}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Serialise the manifest to TOML.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self)
            .map_err(|e| ApexError::config(format!("could not serialise manifest: {e}")))
    }

    /// Validate required fields and basic invariants.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(ApexError::config(
                "agent manifest requires a non-empty 'id'",
            ));
        }
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(ApexError::config(format!(
                "agent id '{}' may only contain letters, digits, '-', '_' and '.'",
                self.id
            )));
        }
        if self.name.trim().is_empty() {
            return Err(ApexError::config(
                "agent manifest requires a non-empty 'name'",
            ));
        }
        if self.version.trim().is_empty() {
            return Err(ApexError::config("agent manifest requires a 'version'"));
        }
        if self.instructions.trim().is_empty() {
            return Err(ApexError::config(
                "agent manifest requires non-empty 'instructions'",
            ));
        }
        Ok(())
    }

    /// Whether a tool is permitted by this manifest's tool policy.
    pub fn allows_tool(&self, name: &str) -> bool {
        if self.tools.denied.iter().any(|t| t == name) {
            return false;
        }
        self.tools.allowed.is_empty() || self.tools.allowed.iter().any(|t| t == name)
    }

    /// Paths this agent declares it may modify.
    pub fn write_scope(&self) -> &[String] {
        &self.writes.paths
    }

    /// Whether this agent declares any write scope at all.
    pub fn declares_write_scope(&self) -> bool {
        !self.writes.paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
id = "apex-default"
name = "Default Engineer"
version = "0.1.0"
description = "Generic coding agent"
maintainer = "APEX"
instructions = "You are a careful software engineer."

[tools]
allowed = ["read_file", "edit_file"]
"#;

    #[test]
    fn parses_and_validates() {
        let manifest = AgentManifest::from_toml(SAMPLE).unwrap();
        assert_eq!(manifest.id, "apex-default");
        assert!(manifest.allows_tool("read_file"));
        assert!(!manifest.allows_tool("run_command"));
    }

    #[test]
    fn rejects_empty_instructions() {
        let bad = "id=\"x\"\nname=\"X\"\nversion=\"1\"\n";
        assert!(AgentManifest::from_toml(bad).is_err());
    }
}
