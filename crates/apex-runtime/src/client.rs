//! Client for talking to a running APEX runtime.
//!
//! Used by both the CLI and the IDE extension. Handles the handshake, request
//! correlation and live event delivery.

use std::collections::HashMap;
use std::sync::Arc;

use apex_core::error::{ApexError, Result};
use apex_protocol::{Event, Frame, Request, Response};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::transport::{connect, read_frame, write_frame, ConnectionInfo, DynStream};

/// A connected runtime client.
pub struct Client {
    writer: tokio::io::WriteHalf<DynStream>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>,
    next_id: Arc<Mutex<u64>>,
    pub events: mpsc::UnboundedReceiver<Event>,
}

impl Client {
    /// Connect to a runtime using the published connection info.
    pub async fn connect(info: &ConnectionInfo) -> Result<Client> {
        let stream = connect(info).await?;
        let (mut reader, writer) = tokio::io::split(stream);
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(Mutex::new(1u64));
        let (tx, rx) = mpsc::unbounded_channel::<Event>();

        // Response reader task.
        {
            let pending = Arc::clone(&pending);
            tokio::spawn(async move {
                loop {
                    match read_frame(&mut reader).await {
                        Ok(Some(Frame::Response { id, response })) => {
                            if let Some(sender) = pending.lock().await.remove(&id) {
                                let _ = sender.send(response);
                            }
                        }
                        Ok(Some(Frame::Event { event })) => {
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => return,
                        Err(_) => return,
                    }
                }
            });
        }

        let mut client = Client {
            writer,
            pending,
            next_id,
            events: rx,
        };
        client.handshake(info.token.clone()).await?;
        Ok(client)
    }

    async fn handshake(&mut self, token: String) -> Result<()> {
        let response = self
            .request(Request::Hello {
                protocol_version: apex_protocol::PROTOCOL_VERSION,
                client_name: "apex-client".into(),
                client_version: env!("CARGO_PKG_VERSION").into(),
                token,
            })
            .await?;
        match response {
            Response::Hello {
                protocol_version,
                runtime_version,
                ..
            } => {
                if protocol_version != apex_protocol::PROTOCOL_VERSION {
                    return Err(ApexError::Protocol(format!(
                        "runtime speaks protocol v{protocol_version} but this client speaks v{}",
                        apex_protocol::PROTOCOL_VERSION
                    )));
                }
                tracing::debug!(runtime_version, "connected to APEX runtime");
                Ok(())
            }
            Response::Error { message, .. } => Err(ApexError::Protocol(format!(
                "handshake rejected: {message}"
            ))),
            other => Err(ApexError::Protocol(format!(
                "unexpected handshake response: {other:?}"
            ))),
        }
    }

    /// Send a request and await its correlated response.
    pub async fn request(&mut self, request: Request) -> Result<Response> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        write_frame(&mut self.writer, &Frame::Request { id, request }).await?;
        let response = rx
            .await
            .map_err(|_| ApexError::Protocol("runtime closed the connection".into()))?;
        match response {
            Response::Error { code, message } => Err(ApexError::Protocol(format!(
                "runtime error [{code}]: {message}"
            ))),
            other => Ok(other),
        }
    }
}
