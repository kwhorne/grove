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

    /// Shared secret clients must present.
    ///
    /// Required unless `--allow-anonymous` is given. It used to default to
    /// empty, which quietly disabled authentication — so the simplest way to
    /// start a server was also the way that let anyone on the internet publish
    /// through it.
    #[arg(long, env = "GROVE_TUNNEL_TOKEN")]
    token: Option<String>,

    /// Run without authentication. Anyone who can reach the control port may
    /// open a tunnel through this server.
    #[arg(long, conflicts_with = "token")]
    allow_anonymous: bool,

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

    // Refuse to start unauthenticated by accident. `--allow-anonymous` is a
    // legitimate choice for a server behind other controls; forgetting the
    // token is not.
    let token = match (args.token, args.allow_anonymous) {
        (Some(t), _) if !t.trim().is_empty() => Some(t),
        (Some(_), _) => anyhow::bail!(
            "--token was empty. Give it a value, or pass --allow-anonymous to run an open server."
        ),
        (None, true) => {
            tracing::warn!(
                "running without authentication: anyone who can reach the control port \
                 can publish through this server"
            );
            None
        }
        (None, false) => anyhow::bail!(
            "no --token given. Set one (GROVE_TUNNEL_TOKEN), or pass --allow-anonymous \
             to deliberately run an open server."
        ),
    };

    let cfg = ServerConfig {
        control_addr: args.control,
        http_addr: args.http,
        domain: args.domain,
        token,
        scheme: args.scheme,
    };
    run(cfg).await
}
