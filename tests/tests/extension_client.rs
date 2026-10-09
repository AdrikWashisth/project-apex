//! Proves the VS Code extension's protocol client works against a real runtime.
//!
//! `client.ts` imports only Node builtins, so it can be driven from plain Node
//! without launching VS Code. This runs that client against a live runtime and
//! asserts the checks pass — which is what catches a client that type-checks but
//! never actually talks to the server.
//!
//! Skipped automatically when Node is unavailable or the extension has not been
//! compiled, so it never becomes a flaky test.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use apex_core::error::Result;
use apex_memory::Store;
use apex_protocol::ExecutionMode;
use apex_runtime::Runtime;
use apex_tests::support::{init_git_repo, offline_config};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <repo>/tests, so the repo root is one level up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests package lives inside the repo")
        .to_path_buf()
}

fn extension_dir() -> PathBuf {
    repo_root().join("extensions").join("vscode")
}

/// Whether the probe can run here at all.
/// Locate a Node executable, falling back to the common install locations when
/// it is not on the inherited PATH.
fn find_node() -> Option<PathBuf> {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    let candidates: Vec<PathBuf> = std::iter::once(PathBuf::from(name))
        .chain(
            [
                "C:/Program Files/nodejs/node.exe",
                "C:/Program Files (x86)/nodejs/node.exe",
            ]
            .iter()
            .map(PathBuf::from),
        )
        .collect();

    for candidate in candidates {
        if candidate.is_absolute() {
            if candidate.exists() {
                return Some(candidate);
            }
        } else if Command::new(&candidate).arg("--version").output().is_ok() {
            return Some(candidate);
        }
    }
    None
}

/// Whether the probe can run here at all.
fn probe_available() -> Option<PathBuf> {
    let probe = extension_dir().join("scripts").join("probe.js");
    let compiled = extension_dir().join("out").join("client.js");
    if !probe.exists() || !compiled.exists() {
        eprintln!(
            "skipping: extension not compiled (run `npm install && npx tsc` in extensions/vscode)"
        );
        return None;
    }
    match find_node() {
        Some(node) => Some(node),
        None => {
            eprintln!("skipping: node is not available");
            None
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_extension_client_talks_to_a_real_runtime() -> Result<()> {
    let Some(node) = probe_available() else {
        return Ok(());
    };

    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();
    init_git_repo(&repo).await?;

    let runtime = Runtime::new(offline_config(), Arc::new(Store::open_in_memory()?))?;
    let (listener, info) =
        apex_runtime::transport::Listener::bind(apex_core::Transport::TcpLoopback).await?;
    *runtime.info.lock().unwrap() = Some(info.clone());
    let server = tokio::spawn(apex_runtime::ipc::serve(Arc::clone(&runtime), listener));

    // Create a task so list_tasks and the subtask/plan paths have real data.
    let _task = runtime.create_task(
        "Protocol probe".into(),
        repo.to_string_lossy().into_owned(),
        None,
        None,
        ExecutionMode::ManualMulti,
    );

    let output = tokio::task::spawn_blocking(move || {
        Command::new(node)
            .arg(extension_dir().join("scripts").join("probe.js"))
            .arg(&info.endpoint)
            .arg(&info.token)
            .current_dir(extension_dir())
            .output()
    })
    .await
    .map_err(|e| apex_core::error::ApexError::Protocol(format!("probe task failed: {e}")))?
    .map_err(|e| apex_core::error::ApexError::Protocol(format!("could not run node: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // Surface the probe's own checks so a pass is visible evidence rather than
    // an assertion the reader has to take on trust.
    eprintln!("--- extension client probe ---\n{stdout}");

    assert!(
        output.status.success(),
        "the extension client probe failed.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    let checks = stdout.lines().filter(|l| l.starts_with("ok ")).count();
    eprintln!("--- {checks} protocol checks passed ---");
    for line in stdout.lines() {
        if line.starts_with("not ok") {
            panic!("extension client check failed: {line}");
        }
    }
    assert!(
        stdout.contains("ok handshake"),
        "the probe never completed its handshake.\n{stdout}"
    );
    assert!(
        stdout.contains("ok request-correlation"),
        "request correlation was not verified.\n{stdout}"
    );
    assert!(
        stdout.contains("ok still-connected"),
        "the connection did not survive the request sequence.\n{stdout}"
    );

    runtime.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    Ok(())
}
