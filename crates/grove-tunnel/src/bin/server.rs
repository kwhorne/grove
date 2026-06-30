//! `grove-tunnel` — the public-facing tunnel server.
//!
//! Deploy on a host with a public IP and a wildcard DNS record
//! (`*.tunnel.example.com`). Put a TLS terminator (Caddy, Cloudflare, …) in
//! front for public HTTPS and set `--scheme https`.

use std::net::SocketAddr;

use clap::Parser;
use grove_tunnel::server::{run, ServerConfig};

#[derive(Parser, Debug)]
#[command(
    name = "grove-tunnel",
    about = "Grove Tunnel server — share local *.test sites publicly"
)]
struct Args {
    /// Wildcard apex domain, e.g. `tunnel.example.com`.
    #[arg(long, env = "GROVE_TUNNEL_DOMAIN")]
    domain: String,

    /// Shared secret clients must present. Omit for an open server (no auth).
    #[arg(long, env = "GROVE_TUNNEL_TOKEN", default_value = "")]
    token: String,

    /// Address clients connect to (control channel).
    #[arg(long, env = "GROVE_TUNNEL_CONTROL", default_value = "0.0.0.0:7000")]
    control: SocketAddr,

    /// Address the public reaches sites on (HTTP).
    #[arg(long, env = "GROVE_TUNNEL_HTTP", default_value = "0.0.0.0:80")]
    http: SocketAddr,

    /// Scheme advertised in public URLs (`http`, or `https` behind a TLS proxy).
    #[arg(long, env = "GROVE_TUNNEL_SCHEME", default_value = "http")]
    scheme: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let cfg = ServerConfig {
        control_addr: args.control,
        http_addr: args.http,
        domain: args.domain,
        token: args.token,
        scheme: args.scheme,
    };
    run(cfg).await
}
