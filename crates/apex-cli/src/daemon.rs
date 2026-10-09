//! Runtime daemon management: start, attach, and stop.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use apex_core::config::Config;
use apex_runtime::client::Client;
use apex_runtime::{ipc, transport, Runtime};

/// How long to wait for a newly spawned runtime to publish its address.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Connect to an already-running runtime, if one exists.
pub async fn try_connect() -> Result<Option<Client>> {
    let info = match transport::read_runtime_info().await {
        Ok(info) => info,
        Err(_) => return Ok(None),
    };
    match Client::connect(&info).await {
        Ok(client) => Ok(Some(client)),
        Err(_) => Ok(None),
    }
}

/// Connect to the runtime, starting it in the background if necessary.
pub async fn connect_or_start(config: &Config) -> Result<Client> {
    if let Some(client) = try_connect().await? {
        return Ok(client);
    }

    if !config.runtime.autostart {
        anyhow::bail!(
            "the APEX runtime is not running and autostart is disabled; start it with `apex runtime`"
        );
    }

    spawn_background(config)?;

    let deadline = std::time::Instant::now() + STARTUP_TIMEOUT;
    while std::time::Instant::now() < deadline {
        if let Some(client) = try_connect().await? {
            return Ok(client);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    anyhow::bail!("timed out waiting for the APEX runtime to start")
}

/// Spawn a detached runtime process running `apex runtime --foreground`.
fn spawn_background(_config: &Config) -> Result<()> {
    let exe = std::env::current_exe().context("cannot locate the apex executable")?;
    let mut command = tokio::process::Command::new(exe);
    command
        .arg("runtime")
        .arg("--foreground")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);

    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .context("failed to start the APEX runtime")?;
    Ok(())
}

/// Run the runtime in the foreground until it shuts down.
pub async fn run_foreground(config: Config) -> Result<()> {
    let store = apex_memory::Store::open(&apex_core::paths::database_path()?)?;
    let store = std::sync::Arc::new(store);
    let runtime = Runtime::new(config.clone(), store)?;

    let (listener, info) = transport::Listener::bind(config.runtime.transport).await?;
    *runtime.info.lock().unwrap() = Some(info.clone());
    transport::write_runtime_info(&info).await?;
    tracing::info!(
        endpoint = %info.endpoint,
        "APEX runtime listening"
    );

    // Idle watchdog.
    {
        let runtime = std::sync::Arc::clone(&runtime);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                if runtime.should_exit_idle() {
                    tracing::info!("idle shutdown");
                    runtime.shutdown();
                    return;
                }
            }
        });
    }

    let result = ipc::serve(std::sync::Arc::clone(&runtime), listener).await;
    let _ = std::fs::remove_file(transport::runtime_info_path()?);
    result.context("runtime failed")
}

/// Ask a running runtime to shut down.
pub async fn stop() -> Result<()> {
    let mut client = try_connect()
        .await?
        .context("the APEX runtime is not running")?;
    use apex_protocol::Request;
    client.request(Request::Shutdown).await?;
    Ok(())
}
