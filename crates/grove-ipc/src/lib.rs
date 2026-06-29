//! Newline-delimited JSON-RPC over a Unix socket / named pipe (PRD §8.1).
//!
//! The CLI and GUI are thin clients; all privileged, stateful work lives in the
//! daemon. Keeping the wire format plain JSON makes `--json` output and
//! elyra-conductor integration trivial — the same `Response` types are reused.

pub mod client;
pub mod protocol;
pub mod transport;

pub use protocol::{Request, Response, ServiceState, SiteStatus};
