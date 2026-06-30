//! Grove Tunnel — a native, self-hostable alternative to Expose/ngrok.
//!
//! Two halves share one wire protocol:
//!
//! * [`client`] powers `grove share`: it registers a local `*.test` site with a
//!   tunnel server and proxies inbound public requests to it.
//! * [`server`] is the public-facing `grove-tunnel` binary you deploy on a host
//!   with a wildcard domain.
//!
//! Requests are multiplexed over a single yamux connection, and HTTP is spoken
//! end-to-end with `hyper`, so request/response bodies stream without buffering
//! and webhooks work out of the box.

pub mod client;
pub mod protocol;
pub mod record;
pub mod server;
mod util;

pub use client::{run as share, ShareConfig};
pub use record::{now_ms, Recorder, RequestRecord};
pub use server::{run as serve, ServerConfig};
