//! IPC: request dispatch, connection handling and live event streaming.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use apex_core::error::{ApexError, Result};
use apex_protocol::{Event, Frame, Request, Response, RUNTIME_VERSION};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::transport::{read_frame, write_frame, DynStream};
use crate::Runtime;

/// Dispatch a request to the runtime.
pub async fn dispatch(runtime: &Arc<Runtime>, request: Request) -> Result<Response> {
    match request {
        Request::Hello {
            protocol_version,
            client_name,
            client_version,
            ..
        } => {
            tracing::debug!(client = %client_name, version = %client_version, "client connected");
            Ok(Response::Hello {
                protocol_version,
                runtime_version: RUNTIME_VERSION.to_string(),
                session_id: apex_core::new_id("sess"),
            })
        }
        Request::CreateTask {
            objective,
            project_root,
            model,
            agent_id,
            mode,
        } => {
            let task = runtime.create_task(objective, project_root, model, agent_id, mode)?;
            Ok(Response::Task { task })
        }
        Request::ListTasks { limit } => {
            let tasks = runtime.store.list_tasks(limit.unwrap_or(50) as usize)?;
            Ok(Response::TaskList { tasks })
        }
        Request::ShowTask { task_id } => {
            let task = runtime
                .store
                .get_task(&task_id)?
                .ok_or_else(|| ApexError::Storage(format!("task {task_id} not found")))?;
            Ok(Response::Task { task })
        }
        Request::SendInstruction {
            task_id,
            instruction,
        } => {
            runtime.send_instruction(&task_id, instruction).await?;
            Ok(Response::Ok)
        }
        Request::TaskEvents { task_id, after_seq } => {
            let after = after_seq.unwrap_or(0);
            let events = runtime.store.events_after(&task_id, after)?;
            let last_seq = events.last().map(|e| e.seq).unwrap_or(after);
            Ok(Response::Events { events, last_seq })
        }
        Request::ListAgents => Ok(Response::Agents {
            agents: runtime.agents(),
        }),
        Request::CancelTask { task_id } => {
            runtime.cancel_task(&task_id)?;
            Ok(Response::Ok)
        }
        Request::ResumeTask { task_id } => {
            runtime.resume_task(&task_id).await?;
            Ok(Response::Ok)
        }
        Request::ResolveApproval {
            task_id,
            approval_id,
            approved,
        } => {
            let _ = task_id;
            if runtime.resolve_approval(&approval_id, approved)? {
                Ok(Response::Ok)
            } else {
                Ok(Response::error(
                    "approval",
                    "no pending approval with that id (it may have already resolved)",
                ))
            }
        }
        Request::GetDiff { task_id } => {
            let diff = runtime.diff_for(&task_id).await?;
            Ok(Response::Diff { diff })
        }
        Request::Verify { task_id } => {
            let (passed, checks) = runtime.verify_for(&task_id).await?;
            Ok(Response::Verification { passed, checks })
        }
        Request::Status => Ok(Response::Status {
            status: runtime.status()?,
        }),
        Request::Shutdown => {
            let runtime = Arc::clone(runtime);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                runtime.shutdown();
            });
            Ok(Response::Ok)
        }
    }
}

/// Accept connections until the runtime shuts down.
pub async fn serve(runtime: Arc<Runtime>, listener: crate::transport::Listener) -> Result<()> {
    let shutdown = runtime.shutdown_token();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            accepted = listener.accept() => {
                match accepted {
                    Ok(stream) => {
                        let runtime = Arc::clone(&runtime);
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(runtime, stream).await {
                                tracing::debug!(error = %e, "connection closed");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }
}

/// Handle a single client connection for its lifetime.
async fn handle_connection(runtime: Arc<Runtime>, stream: DynStream) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(stream);

    // --- Handshake ---
    let first = read_frame(&mut reader)
        .await?
        .ok_or_else(|| ApexError::Protocol("connection closed before handshake".into()))?;
    let Frame::Request {
        id: hello_id,
        request,
    } = first
    else {
        write_frame(
            &mut writer,
            &Frame::Response {
                id: 0,
                response: Response::error("protocol", "expected a request frame"),
            },
        )
        .await?;
        return Ok(());
    };
    let Request::Hello {
        protocol_version,
        client_name,
        client_version,
        token,
    } = request
    else {
        write_frame(
            &mut writer,
            &Frame::Response {
                id: hello_id,
                response: Response::error("protocol", "handshake must be a hello request"),
            },
        )
        .await?;
        return Ok(());
    };

    // Authenticate against the published connection info. The token is copied
    // out of the mutex before any await so the guard is never held across one.
    let expected_token: Option<String> = runtime
        .info
        .lock()
        .unwrap()
        .as_ref()
        .map(|info| info.token.clone());
    if let Some(expected) = expected_token {
        if expected != token {
            write_frame(
                &mut writer,
                &Frame::Response {
                    id: hello_id,
                    response: Response::error("auth", "invalid connection token"),
                },
            )
            .await?;
            return Ok(());
        }
    }

    if protocol_version != apex_protocol::PROTOCOL_VERSION {
        write_frame(
            &mut writer,
            &Frame::Response {
                id: hello_id,
                response: Response::error(
                    "version",
                    format!(
                        "protocol version mismatch: client={protocol_version} runtime={}",
                        apex_protocol::PROTOCOL_VERSION
                    ),
                ),
            },
        )
        .await?;
        return Ok(());
    }

    tracing::info!(client = %client_name, version = %client_version, "client connected");
    runtime.touch();

    write_frame(
        &mut writer,
        &Frame::Response {
            id: hello_id,
            response: Response::Hello {
                protocol_version: apex_protocol::PROTOCOL_VERSION,
                runtime_version: RUNTIME_VERSION.to_string(),
                session_id: apex_core::new_id("sess"),
            },
        },
    )
    .await?;

    // Live events are funnelled through one channel per connection.
    let (live_tx, mut live_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let mut forwarders: HashMap<String, JoinHandle<()>> = HashMap::new();
    let shutdown = runtime.shutdown_token();

    let result = loop {
        tokio::select! {
            _ = shutdown.cancelled() => break Ok(()),

            event = live_rx.recv() => {
                match event {
                    Some(event) => {
                        if write_frame(&mut writer, &Frame::Event { event }).await.is_err() {
                            break Ok(());
                        }
                    }
                    None => break Ok(()),
                }
            }

            frame = read_frame(&mut reader) => {
                match frame {
                    Ok(Some(Frame::Request { id, request })) => {
                        runtime.touch();
                        let subscribe_to = match &request {
                            Request::TaskEvents { task_id, .. } => Some(task_id.clone()),
                            _ => None,
                        };
                        let response = match dispatch(&runtime, request).await {
                            Ok(r) => r,
                            Err(e) => crate::transport::error_response(&e),
                        };
                        if let Some(task_id) = subscribe_to {
                            if let Some(handle) = start_forwarder(&runtime, &task_id, &live_tx) {
                                if let Some(old) = forwarders.insert(task_id, handle) {
                                    old.abort();
                                }
                            }
                        }
                        if write_frame(&mut writer, &Frame::Response { id, response }).await.is_err() {
                            break Ok(());
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break Ok(()),
                    Err(e) => {
                        tracing::debug!(error = %e, "frame read error");
                        break Ok(());
                    }
                }
            }
        }
    };

    for handle in forwarders.values() {
        handle.abort();
    }
    result
}

/// Subscribe to a task's live events and forward them to the connection channel.
fn start_forwarder(
    runtime: &Arc<Runtime>,
    task_id: &str,
    live_tx: &tokio::sync::mpsc::UnboundedSender<Event>,
) -> Option<JoinHandle<()>> {
    let (rx, _last_seq) = runtime.subscribe(task_id).ok()?;
    let tx = live_tx.clone();
    Some(tokio::spawn(async move {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    }))
}
