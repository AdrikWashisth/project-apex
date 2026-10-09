//! Layered configuration for APEX.
//!
//! Configuration is resolved in this order (later layers win):
//! 1. Built-in defaults.
//! 2. User config at `~/.apex/config/config.toml`.
//! 3. Project config at `<project>/.apex/config.toml`.
//! 4. A small set of environment variable overrides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ApexError, Result};
use crate::paths;

/// The resolved runtime configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Default model in `provider/model` form.
    pub default_model: Option<String>,
    /// Configured model providers, keyed by provider name.
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Execution budgets.
    pub budget: Budget,
    /// Permission policy.
    pub permissions: Permissions,
    /// Persistent runtime settings.
    pub runtime: RuntimeConfig,
    /// Memory and retention settings.
    pub memory: MemoryConfig,
}

/// A single model provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider adapter kind.
    pub kind: ProviderKind,
    /// Base URL for the provider API (no trailing slash required).
    pub base_url: String,
    /// Name of the environment variable holding the API key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Known model identifiers offered by this provider.
    #[serde(default)]
    pub models: Vec<String>,
    /// Provider-level default model.
    #[serde(default)]
    pub default_model: Option<String>,
}

impl ProviderConfig {
    /// An OpenAI-compatible provider with sensible defaults.
    pub fn openai_compatible(base_url: impl Into<String>) -> Self {
        ProviderConfig {
            kind: ProviderKind::OpenAiCompatible,
            base_url: base_url.into(),
            api_key_env: Some("OPENAI_API_KEY".into()),
            models: vec!["gpt-4o-mini".into()],
            default_model: Some("gpt-4o-mini".into()),
        }
    }
}

/// Supported provider adapter kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any endpoint exposing the OpenAI Chat Completions API.
    OpenAiCompatible,
    /// A deterministic, offline provider used for tests and local demos.
    Fake,
}

/// Resource budget for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    /// Maximum total model tokens (prompt + completion) for a task.
    pub max_model_tokens: u64,
    /// Maximum estimated spend in USD for a task.
    pub max_cost_usd: f64,
    /// Maximum number of tool calls for a task.
    pub max_tool_calls: u32,
    /// Maximum wall-clock duration in seconds.
    pub max_wall_secs: u64,
    /// Maximum repair attempts during verification.
    pub max_repair_attempts: u32,
    /// Maximum agent reasoning steps.
    pub max_steps: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_model_tokens: 500_000,
            max_cost_usd: 5.0,
            max_tool_calls: 200,
            max_wall_secs: 1800,
            max_repair_attempts: 3,
            max_steps: 60,
        }
    }
}

/// Permission profiles, ordered from least to most capable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionProfile {
    /// Only read-only tools are permitted.
    ReadOnly,
    /// Reads plus writes, with approval prompts for risky operations.
    Assisted,
    /// Autonomous execution within the workspace, still bounded and audited.
    ControlledAutonomous,
    /// User-configured advanced autonomy. Never bypasses the audit trail.
    Advanced,
}

/// Permission policy applied to tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    /// Active permission profile.
    pub profile: PermissionProfile,
    /// Whether network access from tools is allowed.
    pub allow_network: bool,
    /// Whether shell command execution is allowed.
    pub allow_shell: bool,
    /// Additional writable path globs, relative to the workspace root.
    pub writable_paths: Vec<String>,
    /// Risk levels that require explicit human approval.
    pub require_approval: Vec<RiskClass>,
}

impl Default for Permissions {
    fn default() -> Self {
        Permissions {
            profile: PermissionProfile::Assisted,
            allow_network: false,
            allow_shell: true,
            writable_paths: vec!["**".into()],
            require_approval: vec![RiskClass::Destructive],
        }
    }
}

/// Risk classification for a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    /// No side effects outside process memory.
    ReadOnly,
    /// Writes files or mutates workspace state.
    Mutating,
    /// Executes commands or opens network connections.
    Executing,
    /// Potentially destructive or hard to reverse.
    Destructive,
}

/// Persistent runtime transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// Unix-domain socket (Unix) or named pipe (Windows).
    LocalSocket,
    /// Loopback TCP with a per-session bearer token.
    TcpLoopback,
}

/// Persistent runtime configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// Transport used by clients to reach the runtime.
    pub transport: Transport,
    /// Whether the CLI may autostart a background runtime.
    pub autostart: bool,
    /// Seconds of inactivity before an idle runtime exits.
    pub idle_shutdown_secs: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        RuntimeConfig {
            transport: Transport::LocalSocket,
            autostart: true,
            idle_shutdown_secs: 3600,
        }
    }
}

/// Memory and retention configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Whether persistence is enabled.
    pub enabled: bool,
    /// Days to retain completed task history.
    pub retention_days: u32,
    /// Maximum events retained per task.
    pub max_events_per_task: u32,
    /// Maximum total cost allowed before warning.
    pub warn_cost_usd: f64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        MemoryConfig {
            enabled: true,
            retention_days: 90,
            max_events_per_task: 10_000,
            warn_cost_usd: 4.0,
        }
    }
}

impl Config {
    /// Path to the user-level configuration file.
    pub fn user_path() -> Result<PathBuf> {
        Ok(paths::config_dir()?.join("config.toml"))
    }

    /// Path to the project-level configuration file.
    pub fn project_path(project_dir: &Path) -> PathBuf {
        project_dir.join(".apex").join("config.toml")
    }

    /// Load configuration layered on top of built-in defaults.
    pub fn load(project_dir: Option<&Path>) -> Result<Config> {
        let mut merged = toml::Value::Table(toml::map::Map::new());

        if let Ok(user_path) = Self::user_path() {
            if user_path.exists() {
                let layer = read_toml(&user_path)?;
                deep_merge(&mut merged, layer);
            }
        }

        if let Some(dir) = project_dir {
            let project_path = Self::project_path(dir);
            if project_path.exists() {
                let layer = read_toml(&project_path)?;
                deep_merge(&mut merged, layer);
            }
        }

        let mut config: Config = merged
            .try_into()
            .map_err(|e| ApexError::config(format!("invalid configuration: {e}")))?;
        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(model) = std::env::var("APEX_DEFAULT_MODEL") {
            if !model.is_empty() {
                self.default_model = Some(model);
            }
        }
        if let Ok(base) = std::env::var("APEX_BASE_URL") {
            if !base.is_empty() {
                let provider = self
                    .providers
                    .entry("default".into())
                    .or_insert_with(|| ProviderConfig::openai_compatible(base.clone()));
                provider.base_url = base;
            }
        }
        if let Ok(profile) = std::env::var("APEX_PERMISSION_PROFILE") {
            if let Ok(p) = toml::from_str::<PermissionProfile>(&format!("\"{profile}\"")) {
                self.permissions.profile = p;
            }
        }
    }

    /// Validate the configuration for internal consistency.
    pub fn validate(&self) -> Result<()> {
        for (name, provider) in &self.providers {
            if provider.base_url.trim().is_empty() {
                return Err(ApexError::config(format!(
                    "provider '{name}' has an empty base_url"
                )));
            }
        }
        if let Some(model) = &self.default_model {
            let provider = model.split('/').next().unwrap_or("");
            if !provider.is_empty() && !self.providers.contains_key(provider) {
                return Err(ApexError::config(format!(
                    "default_model '{model}' references unknown provider '{provider}'"
                )));
            }
        }
        Ok(())
    }

    /// Persist this configuration to the user-level file.
    pub fn save_user(&self) -> Result<()> {
        let path = Self::user_path()?;
        if let Some(parent) = path.parent() {
            paths::ensure_dir(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| ApexError::config(format!("could not serialize config: {e}")))?;
        std::fs::write(&path, text)?;
        Ok(())
    }

    /// Resolve a provider and model from a `provider/model` reference,
    /// falling back to the default model.
    pub fn resolve_model(&self, reference: Option<&str>) -> Result<(String, String)> {
        let reference = reference
            .map(|s| s.to_string())
            .or_else(|| self.default_model.clone())
            .ok_or_else(|| {
                ApexError::config(
                    "no model selected; run `apex models set <provider/model>` or set APEX_DEFAULT_MODEL",
                )
            })?;

        let (provider, model) = reference.split_once('/').ok_or_else(|| {
            ApexError::config(format!(
                "model reference '{reference}' must be in provider/model form"
            ))
        })?;

        if !self.providers.contains_key(provider) {
            return Err(ApexError::config(format!(
                "unknown provider '{provider}' in model reference '{reference}'"
            )));
        }
        Ok((provider.to_string(), model.to_string()))
    }
}

fn read_toml(path: &Path) -> Result<toml::Value> {
    let text = std::fs::read_to_string(path)?;
    let value: toml::Value = toml::from_str(&text)
        .map_err(|e| ApexError::config(format!("failed to parse {}: {e}", path.display())))?;
    Ok(value)
}

/// Recursively merge `overlay` into `base`. Tables are merged; other values
/// are replaced by the overlay.
fn deep_merge(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}
