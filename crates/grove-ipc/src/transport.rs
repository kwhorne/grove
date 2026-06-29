//! Length-agnostic, newline-delimited JSON framing over an async stream.

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Errors surfaced by the transport layer.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("connection closed before a full message was received")]
    Closed,
}

/// Write one JSON value followed by `\n`.
pub async fn write_message<W, T>(writer: &mut W, msg: &T) -> Result<(), TransportError>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let mut bytes = serde_json::to_vec(msg)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one newline-delimited JSON value.
pub async fn read_message<R, T>(reader: &mut R) -> Result<T, TransportError>
where
    R: AsyncBufReadExt + Unpin,
    T: DeserializeOwned,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Err(TransportError::Closed);
    }
    let value = serde_json::from_str(line.trim_end())?;
    Ok(value)
}

/// Convenience wrapper that buffers a reader half.
pub fn buffered<R: AsyncBufReadExt + Unpin>(reader: R) -> R {
    reader
}

/// Helper to wrap a raw stream's read half in a `BufReader`.
pub fn buf_reader<R: tokio::io::AsyncRead + Unpin>(r: R) -> BufReader<R> {
    BufReader::new(r)
}
