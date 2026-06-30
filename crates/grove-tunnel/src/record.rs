//! Per-request telemetry for the tunnel inspector.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single request/response that flowed through a tunnel.
#[derive(Debug, Clone)]
pub struct RequestRecord {
    /// Wall-clock time the request started (unix milliseconds).
    pub at_unix_ms: u64,
    /// HTTP method, e.g. `GET`.
    pub method: String,
    /// Request path with query, e.g. `/webhooks/stripe?x=1`.
    pub path: String,
    /// Response status code (0 if the local site was unreachable).
    pub status: u16,
    /// Round-trip duration in milliseconds.
    pub duration_ms: u64,
}

/// A sink the client calls once per completed request. Wired by the daemon to a
/// ring buffer, or by the CLI to a live log line.
pub type Recorder = Arc<dyn Fn(RequestRecord) + Send + Sync>;

/// Current unix time in milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
