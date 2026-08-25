//! The public-facing tunnel server. Runs on a host with a wildcard domain
//! (`*.tunnel.example.com`) and a public IP. It accepts control connections
//! from `grove share` clients and relays public HTTP requests to them over
//! multiplexed yamux streams.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::{HeaderValue, AUTHORIZATION, HOST, WWW_AUTHENTICATE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_yamux::{Config as YamuxConfig, Control, Session};

use crate::protocol::{read_msg, write_msg, Hello, Reply};
use crate::util::{text, Body};

/// Configuration for [`run`].
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Where clients connect to register tunnels, e.g. `0.0.0.0:7000`.
    pub control_addr: SocketAddr,
    /// Where the public reaches sites, e.g. `0.0.0.0:80`.
    pub http_addr: SocketAddr,
    /// Wildcard apex, e.g. `tunnel.example.com`.
    pub domain: String,
    /// Shared secret clients must present.
    ///
    /// `None` is an open server, and reaching it now takes an explicit
    /// `--allow-anonymous`. It used to be what you got by leaving `--token`
    /// off, which meant the obvious deployment was the unauthenticated one.
    pub token: Option<String>,
    /// Scheme advertised in public URLs (`http` or, behind a TLS terminator,
    /// `https`).
    pub scheme: String,
}

/// A live tunnel: how to reach the client and how it wants requests shaped.
#[derive(Clone)]
struct Tunnel {
    control: Control,
    local_host: String,
    basic_auth: Option<String>,
}

type Registry = Arc<Mutex<HashMap<String, Tunnel>>>;

/// Run the tunnel server until a fatal error occurs.
pub async fn run(cfg: ServerConfig) -> anyhow::Result<()> {
    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));

    let control = TcpListener::bind(cfg.control_addr).await?;
    let http = TcpListener::bind(cfg.http_addr).await?;
    tracing::info!(control = %cfg.control_addr, http = %cfg.http_addr, domain = %cfg.domain, "tunnel server listening");

    let cfg = Arc::new(cfg);

    // Public HTTP acceptor.
    {
        let registry = registry.clone();
        let domain = cfg.domain.clone();
        let scheme = cfg.scheme.clone();
        tokio::spawn(async move {
            loop {
                match http.accept().await {
                    Ok((stream, peer)) => {
                        let registry = registry.clone();
                        let domain = domain.clone();
                        let scheme = scheme.clone();
                        tokio::spawn(async move {
                            let svc = service_fn(move |req| {
                                handle_public(
                                    req,
                                    registry.clone(),
                                    domain.clone(),
                                    scheme.clone(),
                                    peer,
                                )
                            });
                            if let Err(e) = public_http()
                                .serve_connection(TokioIo::new(stream), svc)
                                .await
                            {
                                tracing::debug!(error = %e, "public connection ended");
                            }
                        });
                    }
                    Err(e) => tracing::warn!(error = %e, "public accept failed"),
                }
            }
        });
    }

    // Control acceptor.
    loop {
        let (stream, peer) = control.accept().await?;
        let registry = registry.clone();
        let cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_control(stream, peer, registry, cfg).await {
                tracing::debug!(error = %e, %peer, "control connection ended");
            }
        });
    }
}

/// Handle one client control connection for its whole lifetime.
async fn handle_control(
    stream: TcpStream,
    peer: SocketAddr,
    registry: Registry,
    cfg: Arc<ServerConfig>,
) -> anyhow::Result<()> {
    let _ = stream.set_nodelay(true);
    let mut session = Session::new_server(stream, YamuxConfig::default());
    let control = session.control();

    // The session must be polled continuously for I/O to flow, so drive it in a
    // task and hand inbound streams back over a channel.
    let (inbound_tx, mut inbound_rx) = tokio::sync::mpsc::channel(16);
    let driver = tokio::spawn(async move {
        while let Some(item) = session.next().await {
            match item {
                Ok(s) => {
                    if inbound_tx.send(s).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // The client opens exactly one stream (the control channel).
    let mut ctrl = match inbound_rx.recv().await {
        Some(s) => s,
        None => {
            driver.abort();
            return Ok(());
        }
    };

    let hello: Hello = read_msg(&mut ctrl).await?;
    if !token_ok(cfg.token.as_deref(), &hello.token) {
        let _ = write_msg(
            &mut ctrl,
            &Reply::Error {
                message: "invalid token".into(),
            },
        )
        .await;
        return Ok(());
    }

    // Assign a subdomain.
    let subdomain = pick_subdomain(&registry, hello.subdomain.clone()).await;
    let public_host = format!("{}.{}", subdomain, cfg.domain);
    let public_url = format!("{}://{}", cfg.scheme, public_host);

    registry.lock().await.insert(
        subdomain.clone(),
        Tunnel {
            control: control.clone(),
            local_host: hello.local_host.clone(),
            basic_auth: hello.basic_auth.clone(),
        },
    );
    tracing::info!(%peer, %public_host, local = %hello.local_host, "tunnel opened");

    write_msg(
        &mut ctrl,
        &Reply::Welcome {
            public_host: public_host.clone(),
            public_url,
        },
    )
    .await?;

    // Keep the session alive until the client disconnects. Reading the control
    // stream returns EOF when the client goes away.
    let mut buf = [0u8; 64];
    loop {
        use tokio::io::AsyncReadExt;
        match ctrl.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {} // client keepalive bytes (ignored)
        }
    }

    registry.lock().await.remove(&subdomain);
    driver.abort();
    tracing::info!(%public_host, "tunnel closed");
    Ok(())
}

/// Pick a free subdomain: honour the requested one when available, else random.
async fn pick_subdomain(registry: &Registry, requested: Option<String>) -> String {
    let map = registry.lock().await;
    if let Some(req) = requested {
        let clean: String = req
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>()
            .to_lowercase();
        let reserved = RESERVED_SUBDOMAINS.contains(&clean.as_str());
        if !clean.is_empty() && !reserved && !map.contains_key(&clean) {
            return clean;
        }
        if reserved {
            tracing::debug!(requested = %clean, "refused a reserved subdomain");
        }
    }
    loop {
        let candidate = random_subdomain();
        if !map.contains_key(&candidate) {
            return candidate;
        }
    }
}

fn random_subdomain() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static CTR: AtomicU64 = AtomicU64::new(0x9E3779B97F4A7C15);
    let mut x = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ CTR.fetch_add(0x9E3779B97F4A7C15, Ordering::Relaxed);
    let mut s = String::with_capacity(8);
    for _ in 0..8 {
        let d = (x % 36) as u32;
        x /= 36;
        s.push(char::from_digit(d, 36).unwrap_or('0'));
    }
    s
}

/// Serve one public HTTP request by relaying it through the matching tunnel.
async fn handle_public(
    mut req: Request<Incoming>,
    registry: Registry,
    domain: String,
    scheme: String,
    peer: SocketAddr,
) -> Result<Response<Body>, hyper::Error> {
    // Caddy on-demand-TLS authorization endpoint: only allow certificates for
    // hostnames under our own domain.
    if req.uri().path() == "/__grove_ask" {
        let ok = req
            .uri()
            .query()
            .and_then(|q| {
                q.split('&')
                    .find_map(|kv| kv.strip_prefix("domain=").map(|v| v.to_string()))
            })
            .map(|d| d == domain || d.ends_with(&format!(".{domain}")))
            .unwrap_or(false);
        return Ok(text(
            if ok {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            if ok { "ok\n" } else { "no\n" },
        ));
    }

    let host = req
        .headers()
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h).to_string())
        .unwrap_or_default();
    let Some(subdomain) = subdomain_of(&host, &domain) else {
        return Ok(text(StatusCode::NOT_FOUND, "No such tunnel.\n"));
    };

    let tunnel = match registry.lock().await.get(&subdomain).cloned() {
        Some(t) => t,
        None => return Ok(text(StatusCode::NOT_FOUND, "No such tunnel.\n")),
    };

    // Optional HTTP Basic auth.
    if let Some(expected) = &tunnel.basic_auth {
        if !basic_auth_ok(&req, expected) {
            let mut resp = text(StatusCode::UNAUTHORIZED, "Authentication required.\n");
            resp.headers_mut().insert(
                WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"Grove Tunnel\""),
            );
            return Ok(resp);
        }
    }

    // Keep the public Host so the app generates correct (public) URLs; carry the
    // local site name for routing and advertise the public scheme.
    if let Ok(hv) = HeaderValue::from_str(&tunnel.local_host) {
        req.headers_mut().insert("x-grove-site", hv);
    }
    if let Ok(hv) = HeaderValue::from_str(&scheme) {
        req.headers_mut().insert("x-forwarded-proto", hv);
    }
    // Overwrite rather than append: this *is* the edge, so any inbound
    // `X-Forwarded-For` was written by the caller. Passing it through let anyone
    // tell the app behind the tunnel which IP they were coming from — enough to
    // slip past an allow-list or poison a rate limiter.
    if let Ok(hv) = HeaderValue::from_str(&peer.ip().to_string()) {
        req.headers_mut().insert("x-forwarded-for", hv);
    }

    match relay(req, tunnel).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            tracing::debug!(error = %e, "relay failed");
            Ok(text(
                StatusCode::BAD_GATEWAY,
                "Tunnel client unreachable.\n",
            ))
        }
    }
}

/// Open a fresh yamux stream to the client and run one HTTP exchange over it.
async fn relay(req: Request<Incoming>, tunnel: Tunnel) -> anyhow::Result<Response<Body>> {
    let mut control = tunnel.control;
    let stream = control.open_stream().await?;
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake::<_, Incoming>(TokioIo::new(stream)).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let resp = sender.send_request(req).await?;
    Ok(resp.map(|b| b.boxed()))
}

/// An HTTP/1 server with a header-read deadline.
///
/// A bare builder waits forever for request headers, so a handful of sockets
/// that connect and dribble a byte at a time can hold the server open
/// indefinitely — slowloris. The daemon's own proxy already sets this; the
/// tunnel, which is the part actually exposed to the internet, did not.
///
/// The timer is not optional: hyper panics on the first connection that reaches
/// the deadline if a timeout is configured without one.
fn public_http() -> http1::Builder {
    let mut builder = http1::Builder::new();
    builder.timer(TokioTimer::new());
    builder.header_read_timeout(HEADER_READ_TIMEOUT);
    builder
}

/// How long a client may take to send its request headers.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(15);

/// Subdomain names a client may not claim.
///
/// Not a security boundary on its own — with a token, whoever can connect can
/// pick a name — but these are the ones that impersonate the service itself or
/// its operator, and there is no reason to hand them out.
const RESERVED_SUBDOMAINS: &[&str] = &[
    "www",
    "api",
    "admin",
    "mail",
    "smtp",
    "ftp",
    "ns",
    "ns1",
    "ns2",
    "mx",
    "autodiscover",
    "static",
    "assets",
    "cdn",
    "status",
    "dashboard",
    "login",
    "account",
    "billing",
    "grove",
    "tunnel",
];

/// The subdomain a public request is for, given the server's own domain.
///
/// Requires the host to sit *under* the domain. The previous rule took the
/// first label of whatever `Host` said, so a request to the apex
/// (`tunnel.example.com`) yielded the subdomain `tunnel` — meaning a client that
/// claimed `tunnel` captured traffic aimed at the service itself. Anything that
/// is not `<something>.<domain>` now matches no tunnel at all.
fn subdomain_of(host: &str, domain: &str) -> Option<String> {
    let host = host.split(':').next().unwrap_or(host).to_lowercase();
    let domain = domain.to_lowercase();
    let prefix = host.strip_suffix(&format!(".{domain}"))?;
    if prefix.is_empty() {
        return None;
    }
    Some(prefix.to_string())
}

/// Compare a presented token against the configured one in constant time.
///
/// `None` is an open server. A plain `!=` leaks the length of the shared prefix
/// to anyone who can measure a few thousand handshakes.
fn token_ok(configured: Option<&str>, presented: &str) -> bool {
    let Some(expected) = configured else {
        return true;
    };
    ct_eq(expected.as_bytes(), presented.as_bytes())
}

/// Constant-time byte equality.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    // Lengths differing is not itself secret — the comparison below needs equal
    // lengths, and `ct_eq` on slices of different lengths is not defined.
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

fn basic_auth_ok(req: &Request<Incoming>, expected: &str) -> bool {
    let Some(val) = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(b64) = val.strip_prefix("Basic ") else {
        return false;
    };
    let Some(decoded) = base64_decode(b64.trim()) else {
        return false;
    };
    ct_eq(&decoded, expected.as_bytes())
}

/// Minimal standard base64 decoder (no external crate).
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        let mut bits = 0;
        for &c in chunk {
            acc = (acc << 6) | val(c)? as u32;
            bits += 6;
        }
        acc <<= 32 - bits;
        let nbytes = bits / 8;
        for i in 0..nbytes {
            out.push((acc >> (24 - i * 8)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The apex hijack. The old rule took the first label of `Host`, so a
    /// request to the server's own domain yielded the subdomain `tunnel` — and a
    /// client that claimed `tunnel` captured traffic meant for the service.
    #[test]
    fn the_apex_belongs_to_no_tunnel() {
        for host in [
            "tunnel.example.com",
            "tunnel.example.com:80",
            "TUNNEL.EXAMPLE.COM",
        ] {
            assert_eq!(
                subdomain_of(host, "tunnel.example.com"),
                None,
                "{host} must not resolve to a tunnel"
            );
        }
    }

    #[test]
    fn a_subdomain_of_our_domain_resolves() {
        assert_eq!(
            subdomain_of("abc123.tunnel.example.com", "tunnel.example.com").as_deref(),
            Some("abc123")
        );
        // Case and port are not part of the name.
        assert_eq!(
            subdomain_of("ABC123.tunnel.example.com:8080", "tunnel.example.com").as_deref(),
            Some("abc123")
        );
    }

    /// A host that merely *ends with* our domain's text, or is somewhere else
    /// entirely, is not ours.
    #[test]
    fn foreign_hosts_resolve_to_nothing() {
        for host in [
            "eviltunnel.example.com",
            "tunnel.example.com.evil.test",
            "example.com",
            "localhost",
            "",
        ] {
            assert_eq!(
                subdomain_of(host, "tunnel.example.com"),
                None,
                "{host} must not resolve to a tunnel"
            );
        }
    }

    #[test]
    fn an_open_server_accepts_anything_and_a_closed_one_does_not() {
        assert!(token_ok(None, ""), "an open server takes any token");
        assert!(token_ok(None, "whatever"));

        assert!(token_ok(Some("s3cret"), "s3cret"));
        for wrong in ["", "s3cre", "s3cret ", "S3CRET", "s3cretx"] {
            assert!(!token_ok(Some("s3cret"), wrong), "{wrong:?} was accepted");
        }
    }

    #[test]
    fn constant_time_equality_still_gets_the_answer_right() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"), "differing lengths are not equal");
        assert!(ct_eq(b"", b""));
    }

    /// Reserved names exist so a client cannot pose as the service or its
    /// operator. Pinned as a list so removing one is a deliberate act.
    #[test]
    fn service_names_are_reserved() {
        for name in ["www", "api", "admin", "grove", "tunnel", "status"] {
            assert!(
                RESERVED_SUBDOMAINS.contains(&name),
                "{name} should be reserved"
            );
        }
    }

    #[tokio::test]
    async fn a_reserved_name_is_not_handed_out() {
        let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
        let picked = pick_subdomain(&registry, Some("admin".into())).await;
        assert_ne!(picked, "admin", "a reserved name must not be claimable");
        assert!(!picked.is_empty());
    }

    #[tokio::test]
    async fn a_free_name_is_still_honoured() {
        let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
        assert_eq!(
            pick_subdomain(&registry, Some("my-app".into())).await,
            "my-app"
        );
    }
}
