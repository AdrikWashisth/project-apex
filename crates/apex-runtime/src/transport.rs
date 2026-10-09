//! Local transport: address binding, connection info, and frame I/O.
//!
//! On Unix we use a Unix-domain socket. On other platforms a loopback TCP
//! listener bound to 127.0.0.1 with a random per-runtime token is used. The
//! runtime address is published to a connection-info file so CLI and IDE
//! clients can discover it.

use std::path::PathBuf;

use apex_core::error::{ApexError, Result};
use apex_core::paths;
use apex_core::Transport;
use apex_protocol::{Frame, Response};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum wire frame size (16 MiB).
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// How clients reach the runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub transport: Transport,
    pub endpoint: String,
    pub token: String,
}

/// Marker trait for a boxed duplex stream usable by the IPC layer.
pub trait DuplexStream: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> DuplexStream for T {}

/// A boxed duplex I/O stream (TCP or Unix socket based on the transport).
pub type DynStream = Box<dyn DuplexStream + Unpin + Send>;

/// A bound local listener.
pub enum Listener {
    Tcp(tokio::net::TcpListener),
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
}

impl Listener {
    /// Bind a listener according to the configured transport.
    pub async fn bind(transport: Transport) -> Result<(Listener, ConnectionInfo)> {
        #[cfg(unix)]
        if transport == Transport::LocalSocket {
            let path = socket_path()?;
            let _ = std::fs::remove_file(&path);
            let listener = tokio::net::UnixListener::bind(&path)?;
            let info = ConnectionInfo {
                transport,
                endpoint: path.to_string_lossy().into_owned(),
                token: apex_core::new_id("tok"),
            };
            return Ok((Listener::Unix(listener), info));
        }

        if transport == Transport::LocalSocket {
            tracing::warn!(
                "named pipes are not yet implemented on this platform; using loopback TCP"
            );
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let info = ConnectionInfo {
            transport: Transport::TcpLoopback,
            endpoint: addr.to_string(),
            token: apex_core::new_id("tok"),
        };
        Ok((Listener::Tcp(listener), info))
    }

    /// Accept the next connection.
    pub async fn accept(&self) -> Result<DynStream> {
        match self {
            Listener::Tcp(listener) => {
                let (stream, _) = listener.accept().await?;
                Ok(Box::new(stream))
            }
            #[cfg(unix)]
            Listener::Unix(listener) => {
                let (stream, _) = listener.accept().await?;
                Ok(Box::new(stream))
            }
        }
    }

    /// The published connection info for the bound listener.
    pub fn info(&self, transport: Transport, token: String) -> Result<ConnectionInfo> {
        let endpoint = match self {
            Listener::Tcp(listener) => listener.local_addr()?.to_string(),
            #[cfg(unix)]
            Listener::Unix(_) => socket_path()?.to_string_lossy().into_owned(),
        };
        Ok(ConnectionInfo {
            transport,
            endpoint,
            token,
        })
    }
}

#[cfg(unix)]
fn socket_path() -> Result<PathBuf> {
    Ok(paths::state_dir()?.join("apex.sock"))
}

/// File holding the runtime connection info for client discovery.
pub fn runtime_info_path() -> Result<PathBuf> {
    Ok(paths::state_dir()?.join("runtime.json"))
}

/// Publish the connection info for this runtime.
pub async fn write_runtime_info(info: &ConnectionInfo) -> Result<()> {
    let path = runtime_info_path()?;
    let text = serde_json::to_string_pretty(info)?;
    tokio::fs::write(&path, text).await?;
    Ok(())
}

/// Read the connection info published by a running runtime.
pub async fn read_runtime_info() -> Result<ConnectionInfo> {
    let path = runtime_info_path()?;
    let text = tokio::fs::read_to_string(&path).await.map_err(|e| {
        ApexError::config(format!(
            "cannot read runtime info ({}): {e}. Is the APEX runtime running?",
            path.display()
        ))
    })?;
    let info: ConnectionInfo = serde_json::from_str(&text)?;
    Ok(info)
}

/// Connect to a running runtime.
pub async fn connect(info: &ConnectionInfo) -> Result<DynStream> {
    match info.transport {
        #[cfg(unix)]
        Transport::LocalSocket => {
            let stream = tokio::net::UnixStream::connect(&info.endpoint).await?;
            Ok(Box::new(stream))
        }
        _ => {
            let stream = tokio::net::TcpStream::connect(&info.endpoint).await?;
            Ok(Box::new(stream))
        }
    }
}

/// Read one newline-delimited JSON frame.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Frame>> {
    let mut buf = Vec::with_capacity(256);
    loop {
        let byte = match reader.read_u8().await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && buf.is_empty() => {
                return Ok(None)
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if byte == b'\n' {
            break;
        }
        buf.push(byte);
        if buf.len() > MAX_FRAME_BYTES {
            return Err(ApexError::Protocol("frame exceeds size limit".into()));
        }
    }
    let text = String::from_utf8(buf)
        .map_err(|_| ApexError::Protocol("malformed UTF-8 in frame".into()))?;
    let frame: Frame = serde_json::from_str(&text)
        .map_err(|e| ApexError::Protocol(format!("invalid frame: {e}")))?;
    Ok(Some(frame))
}

/// Write one newline-delimited JSON frame.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> Result<()> {
    let mut text = serde_json::to_string(frame)?;
    text.push('\n');
    writer.write_all(text.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Convert an error to a protocol response.
pub fn error_response(error: &ApexError) -> Response {
    use apex_core::error::ApexError as E;
    let (code, message) = match error {
        E::Config(m) => ("config", m.clone()),
        E::Project(m) => ("project", m.clone()),
        E::Git(m) => ("git", m.clone()),
        E::Model { message, .. } => ("model", message.clone()),
        E::Tool { message, .. } => ("tool", message.clone()),
        E::Permission(m) => ("permission", m.clone()),
        E::PathEscape { .. } => ("path_escape", error.to_string()),
        E::Budget(m) => ("budget", m.clone()),
        E::Protocol(m) => ("protocol", m.clone()),
        E::Storage(m) => ("storage", m.clone()),
        E::Cancelled => ("cancelled", "task was cancelled".into()),
        E::Io(e) => ("io", e.to_string()),
        E::Json(e) => ("json", e.to_string()),
    };
    Response::error(code, message)
}
