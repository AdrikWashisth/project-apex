//! Shared helpers for APEX integration tests.

use apex_core::config::{Config, ProviderConfig, ProviderKind};
use apex_core::error::Result;
use std::path::Path;

/// Create a configuration wired to a deterministic offline provider.
pub fn offline_config() -> Config {
    let mut providers = std::collections::BTreeMap::new();
    providers.insert(
        "scripted".to_string(),
        ProviderConfig {
            kind: ProviderKind::Fake,
            base_url: "http://local".into(),
            api_key_env: None,
            models: vec!["model".into()],
            default_model: Some("model".into()),
        },
    );
    Config {
        default_model: Some("scripted/model".into()),
        providers,
        ..Default::default()
    }
}

/// Initialise a real Git repository in `dir` with one commit.
pub async fn init_git_repo(dir: &Path) -> Result<()> {
    let run = |args: &[&str]| {
        let mut command = tokio::process::Command::new("git");
        command.args(args).current_dir(dir);
        async move { command.status().await }
    };
    run(&["init", "-q"]).await.ok();
    run(&["config", "user.email", "apex@test.local"]).await.ok();
    run(&["config", "user.name", "APEX Test"]).await.ok();
    std::fs::write(dir.join("README.md"), "# demo\n")?;
    run(&["add", "-A"]).await.ok();
    run(&["commit", "-q", "-m", "init"]).await.ok();
    Ok(())
}
