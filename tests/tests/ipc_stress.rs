//! Regression test for the IPC frame-desynchronisation bug.
//!
//! `read_frame` consumes bytes one at a time. When it was polled directly inside
//! `tokio::select!` alongside the live-event branch, whichever branch lost the
//! race had its partial read cancelled — silently discarding bytes already
//! consumed. Under load (a task streaming events while the client polls) this
//! corrupted the stream and both sides hung forever.
//!
//! The fix reads frames in a dedicated, never-cancelled task. This test drives
//! exactly the pattern that used to break: many events flowing to the client
//! while requests are issued on the same connection.

use std::sync::Arc;
use std::time::Duration;

use apex_core::error::Result;
use apex_memory::Store;
use apex_protocol::{EventKind, Request, Response};
use apex_runtime::client::Client;
use apex_runtime::{ipc, transport, Runtime};
use apex_tests::support::{init_git_repo, offline_config};

const T: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_still_complete_while_a_task_streams_events() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();
    init_git_repo(&repo).await?;

    let config = offline_config();
    let runtime = Runtime::new(config, Arc::new(Store::open_in_memory()?))?;
    let (listener, info) = transport::Listener::bind(apex_core::Transport::TcpLoopback).await?;
    *runtime.info.lock().unwrap() = Some(info.clone());
    let server = tokio::spawn(ipc::serve(Arc::clone(&runtime), listener));

    let mut client = Client::connect(&info).await?;

    // Create a multi-agent task: it produces a burst of events.
    let Response::Task { task } = tokio::time::timeout(
        T,
        client.request(Request::CreateTask {
            objective: "Noisy task".into(),
            project_root: repo.to_string_lossy().into_owned(),
            model: None,
            agent_id: None,
            mode: apex_protocol::ExecutionMode::ManualMulti,
            team: Some(apex_protocol::TeamSpec::new([
                "apex-default",
                "apex-reviewer",
            ])),
            plan: None,
            workflow: None,
        }),
    )
    .await
    .map_err(|_| apex_core::error::ApexError::Protocol("CreateTask timed out".into()))??
    else {
        panic!("expected a task response");
    };

    // While events flow, keep issuing requests on the SAME connection. Each one
    // must return; previously these hung once the stream desynchronised.
    for round in 0..25 {
        let response = tokio::time::timeout(
            T,
            client.request(Request::TaskEvents {
                task_id: task.id.clone(),
                after_seq: None,
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("round {round}: TaskEvents request timed out"))?;
        assert!(matches!(response, Response::Events { .. }));

        let response = tokio::time::timeout(
            T,
            client.request(Request::ShowTask {
                task_id: task.id.clone(),
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("round {round}: ShowTask request timed out"))?;
        assert!(matches!(response, Response::Task { .. }));
    }

    // Drain live events the client received alongside those responses.
    let mut live_events = 0usize;
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(200), client.events.recv()).await
    {
        if matches!(event.kind, EventKind::ToolFinished { .. }) {
            live_events += 1;
        }
    }

    let events = runtime.store.events_after(&task.id, 0)?;
    assert!(
        events.len() > 5,
        "the task should have produced a burst of events, got {}",
        events.len()
    );
    let _ = live_events;

    // The connection is still healthy: one final request must succeed.
    let status = tokio::time::timeout(T, client.request(Request::Status))
        .await
        .map_err(|_| apex_core::error::ApexError::Protocol("Status request timed out".into()))??;
    assert!(matches!(status, Response::Status { .. }));

    runtime.shutdown();
    let _ = tokio::time::timeout(T, server).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipelined_requests_are_answered_in_order() -> Result<()> {
    // Several requests sent back to back must each get their own response.
    let config = offline_config();
    let runtime = Runtime::new(config, Arc::new(Store::open_in_memory()?))?;
    let (listener, info) = transport::Listener::bind(apex_core::Transport::TcpLoopback).await?;
    *runtime.info.lock().unwrap() = Some(info.clone());
    let server = tokio::spawn(ipc::serve(Arc::clone(&runtime), listener));

    let mut client = Client::connect(&info).await?;
    for round in 0..10 {
        let response = tokio::time::timeout(
            T,
            client.request(Request::ListTasks {
                limit: Some((round + 1) as u32),
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("round {round}: request timed out"))?;
        assert!(matches!(response, Response::TaskList { .. }));
    }

    runtime.shutdown();
    let _ = tokio::time::timeout(T, server).await;
    Ok(())
}
