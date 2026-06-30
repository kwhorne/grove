//! The tiny control-channel protocol spoken between `grove share` (client) and
//! the `grove-tunnel` server. Messages are length-prefixed JSON sent over the
//! first yamux stream the client opens.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum control-message size (1 MiB) — a guard against malicious peers.
const MAX_MSG: usize = 1 << 20;

/// Sent by the client immediately after connecting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// Shared secret; must match the server's token.
    pub token: String,
    /// Desired subdomain. `None` (or taken) → the server assigns a random one.
    pub subdomain: Option<String>,
    /// The `Host` header to use when proxying to the local site,
    /// e.g. `elyra-web.test`.
    pub local_host: String,
    /// Optional HTTP Basic-auth the server should enforce on the public URL,
    /// formatted as `user:pass`.
    pub basic_auth: Option<String>,
}

/// The server's response to a [`Hello`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Reply {
    /// The tunnel is live at this public host.
    Welcome {
        public_host: String,
        public_url: String,
    },
    /// The handshake was rejected.
    Error { message: String },
}

/// Write a length-prefixed JSON message.
pub async fn write_msg<W, T>(w: &mut W, msg: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let buf = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
    if buf.len() > MAX_MSG {
        return Err(std::io::Error::other("control message too large"));
    }
    w.write_u32(buf.len() as u32).await?;
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

/// Read a length-prefixed JSON message.
pub async fn read_msg<R, T>(r: &mut R) -> std::io::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let len = r.read_u32().await? as usize;
    if len > MAX_MSG {
        return Err(std::io::Error::other("control message too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map_err(std::io::Error::other)
}
