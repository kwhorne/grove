//! Thin client used by the CLI/GUI to talk to the daemon over a Unix socket.

use std::path::Path;

use crate::protocol::{Request, Response};
use crate::transport::{self, TransportError};

/// Send a single request to the daemon and await its response.
///
/// On non-Unix platforms a named pipe would be used; that is wired up in
/// `grove-os`. Here we keep the Unix-socket happy path.
#[cfg(unix)]
pub async fn send(socket: &Path, request: &Request) -> Result<Response, TransportError> {
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(socket).await?;
    let (read_half, mut write_half) = stream.into_split();
    transport::write_message(&mut write_half, request).await?;

    let mut reader = transport::buf_reader(read_half);
    let response: Response = transport::read_message(&mut reader).await?;
    Ok(response)
}

#[cfg(not(unix))]
pub async fn send(_socket: &Path, _request: &Request) -> Result<Response, TransportError> {
    Err(TransportError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "named-pipe IPC not yet implemented on this platform",
    )))
}

/// True if a daemon appears to be listening on the socket.
#[cfg(unix)]
pub async fn is_running(socket: &Path) -> bool {
    tokio::net::UnixStream::connect(socket).await.is_ok()
}

#[cfg(not(unix))]
pub async fn is_running(_socket: &Path) -> bool {
    false
}
