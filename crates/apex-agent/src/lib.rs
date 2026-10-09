//! Agent definitions, manifests and the bounded execution loop.

pub mod builtin;
pub mod manifest;
pub mod runner;

use std::path::Path;

use apex_core::error::{ApexError, Result};

pub use builtin::builtin_agents;
pub use manifest::AgentManifest;
pub use runner::{
    AgentOutcome, AgentRunner, ApprovalRequest, Approver, AutoApprover, DenyAllApprover, RunInput,
};

/// Load a single agent manifest from a file.
pub fn load_agent(path: &Path) -> Result<AgentManifest> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        ApexError::config(format!(
            "could not read agent manifest {}: {e}",
            path.display()
        ))
    })?;
    AgentManifest::from_toml(&text)
}

/// Load every `.toml` agent manifest in a directory.
pub fn load_agents_dir(dir: &Path) -> Result<Vec<AgentManifest>> {
    let mut agents = Vec::new();
    if !dir.exists() {
        return Ok(agents);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            match load_agent(&path) {
                Ok(manifest) => agents.push(manifest),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "skipping invalid agent manifest")
                }
            }
        }
    }
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(agents)
}

/// A catalog combining built-in and installed agents.
pub struct AgentCatalog {
    agents: Vec<AgentManifest>,
}

impl AgentCatalog {
    /// Build a catalog from built-in agents plus any installed manifests.
    pub fn load(installed_dir: Option<&Path>) -> Result<AgentCatalog> {
        let mut agents = builtin_agents();
        if let Some(dir) = installed_dir {
            for installed in load_agents_dir(dir)? {
                if let Some(existing) = agents.iter_mut().find(|a| a.id == installed.id) {
                    *existing = installed;
                } else {
                    agents.push(installed);
                }
            }
        }
        Ok(AgentCatalog { agents })
    }

    /// Look up an agent by id.
    pub fn get(&self, id: &str) -> Option<&AgentManifest> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// All agents.
    pub fn all(&self) -> &[AgentManifest] {
        &self.agents
    }

    /// The default agent.
    pub fn default_agent(&self) -> &AgentManifest {
        self.get("apex-default")
            .or_else(|| self.agents.first())
            .expect("at least one built-in agent exists")
    }
}
