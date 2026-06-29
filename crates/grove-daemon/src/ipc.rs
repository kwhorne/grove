//! IPC listener: accepts CLI/GUI connections on a Unix socket and dispatches
//! one request per connection.

use std::path::PathBuf;
use std::sync::Arc;

use grove_ipc::protocol::{Request, Response};
use grove_ipc::transport;

use crate::commands;
use crate::state::DaemonState;

#[cfg(unix)]
pub async fn serve(socket: PathBuf, state: Arc<DaemonState>) -> anyhow::Result<()> {
    use tokio::net::UnixListener;

    // Remove a stale socket from a previous run.
    let _ = std::fs::remove_file(&socket);
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&socket)?;
    tracing::info!(socket = %socket.display(), "IPC listening");

    let shutdown = state.shutdown.clone();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _addr) = accepted?;
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, state).await {
                        tracing::debug!(error = %e, "IPC connection error");
                    }
                });
            }
            _ = shutdown.notified() => {
                tracing::info!("shutdown requested, stopping IPC listener");
                break;
            }
        }
    }
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

#[cfg(unix)]
async fn handle_conn(
    stream: tokio::net::UnixStream,
    state: Arc<DaemonState>,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = transport::buf_reader(read_half);

    // One request/response per connection keeps the protocol trivial.
    let request: Request = match transport::read_message(&mut reader).await {
        Ok(r) => r,
        Err(transport::TransportError::Closed) => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    let response: Response = commands::dispatch(&state, request).await;
    transport::write_message(&mut write_half, &response).await?;
    Ok(())
}

#[cfg(not(unix))]
pub async fn serve(_socket: PathBuf, _state: Arc<DaemonState>) -> anyhow::Result<()> {
    anyhow::bail!("named-pipe IPC not yet implemented on this platform");
}
