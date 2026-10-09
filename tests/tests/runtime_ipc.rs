//! Integration test for the persistent runtime over a real local socket.
//!
//! This exercises the actual IPC path: bind a listener, start the server,
//! connect a client, complete the handshake, create a task, stream its events,
//! and reconnect to replay history. It proves that the CLI and any IDE client
//! really do share one runtime.

use std::sync::Arc;
use std::time::Duration;

use apex_core::error::Result;
use apex_memory::Store;
use apex_protocol::{Frame, Request, Response};
use apex_runtime::client::Client;
use apex_runtime::{ipc, transport, Runtime};
use apex_tests::support::{init_git_repo, offline_config};

fn store() -> Result<Arc<Store>> {
    Ok(Arc::new(Store::open_in_memory()?))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_connects_over_real_socket_and_receives_events() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .with_test_writer()
        .try_init();

    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();
    init_git_repo(&repo).await?;

    let config = offline_config();
    let runtime = Runtime::new(config, store()?)?;
    let (listener, info) = transport::Listener::bind(config_runtime_transport()).await?;
    *runtime.info.lock().unwrap() = Some(info.clone());

    let server = tokio::spawn(ipc::serve(Arc::clone(&runtime), listener));

    // Connect a real client over the real transport.
    let mut client = Client::connect(&info).await?;

    // The runtime knows its agents.
    let agents = client.request(Request::ListAgents).await?;
    match agents {
        Response::Agents { agents } => {
            assert!(!agents.is_empty(), "runtime should expose built-in agents");
            assert!(agents.iter().any(|a| a.id == "apex-default"));
        }
        other => panic!("unexpected response: {other:?}"),
    }

    // Create a real task through the protocol.
    let response = client
        .request(Request::CreateTask {
            objective: "Say hello over IPC".into(),
            project_root: repo.to_string_lossy().into_owned(),
            model: None,
            agent_id: None,
            mode: Default::default(),
            team: None,
            plan: None,
            workflow: None,
        })
        .await?;
    let Response::Task { task } = response else {
        panic!("expected a task response");
    };

    // Poll until the task is terminal, as the CLI does.
    let finished = wait_terminal(&mut client, &task.id, Duration::from_secs(60)).await?;
    assert!(finished.status.is_terminal());

    // Events were produced and persisted.
    let Response::Events { events, last_seq } = client
        .request(Request::TaskEvents {
            task_id: task.id.clone(),
            after_seq: Some(0),
        })
        .await?
    else {
        panic!("expected events");
    };
    assert!(!events.is_empty(), "task should have produced events");
    assert!(last_seq > 0);

    // --- Reconnect: a fresh client replays history from sequence 0 ---
    let mut reconnect = Client::connect(&info).await?;
    let Response::Events { events: replay, .. } = reconnect
        .request(Request::TaskEvents {
            task_id: task.id.clone(),
            after_seq: Some(0),
        })
        .await?
    else {
        panic!("expected events on reconnect");
    };
    assert_eq!(
        replay.len(),
        events.len(),
        "reconnect replays the same history"
    );

    // Diff and verification are addressable over the protocol.
    let _ = reconnect
        .request(Request::GetDiff {
            task_id: task.id.clone(),
        })
        .await?;
    let _ = reconnect
        .request(Request::Verify {
            task_id: task.id.clone(),
        })
        .await?;

    // Status works.
    let response = reconnect.request(Request::Status).await?;
    assert!(matches!(response, Response::Status { .. }));

    // Clean shutdown.
    runtime.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_rejects_a_bad_token() -> Result<()> {
    let config = offline_config();
    let runtime = Runtime::new(config, store()?)?;
    let (listener, info) = transport::Listener::bind(config_runtime_transport()).await?;
    *runtime.info.lock().unwrap() = Some(info.clone());
    let server = tokio::spawn(ipc::serve(Arc::clone(&runtime), listener));

    let mut bad = info.clone();
    bad.token = "not-the-right-token".into();
    // The handshake must fail with an auth error.
    let result = Client::connect(&bad).await;
    assert!(result.is_err(), "a wrong token must be rejected");

    runtime.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    Ok(())
}

fn config_runtime_transport() -> apex_core::Transport {
    // Bind loopback explicitly so the test never depends on platform specifics.
    apex_core::Transport::TcpLoopback
}

async fn wait_terminal(
    client: &mut Client,
    task_id: &str,
    timeout: Duration,
) -> Result<apex_protocol::Task> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let Response::Task { task } = client
            .request(Request::ShowTask {
                task_id: task_id.to_string(),
            })
            .await?
        else {
            panic!("expected task");
        };
        if task.status.is_terminal() {
            return Ok(*task);
        }
        if std::time::Instant::now() > deadline {
            return Err(apex_core::error::ApexError::Storage(format!(
                "timed out waiting for task {task_id}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_and_response_frames_round_trip_as_json() -> Result<()> {
    // The wire format is newline-delimited JSON; assert both directions parse.
    let request = Frame::Request {
        id: 7,
        request: Request::Hello {
            protocol_version: apex_protocol::PROTOCOL_VERSION,
            client_name: "test".into(),
            client_version: "1".into(),
            token: "abc".into(),
        },
    };
    let text = serde_json::to_string(&request)?;
    let parsed: Frame = serde_json::from_str(&text)?;
    match parsed {
        Frame::Request { id, request } => {
            assert_eq!(id, 7);
            match request {
                Request::Hello {
                    protocol_version,
                    token,
                    ..
                } => {
                    assert_eq!(protocol_version, apex_protocol::PROTOCOL_VERSION);
                    assert_eq!(token, "abc");
                }
                other => panic!("unexpected request: {other:?}"),
            }
        }
        other => panic!("unexpected frame: {other:?}"),
    }
    Ok(())
}
