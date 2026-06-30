//! The `grove share` client. Connects to a tunnel server, registers a site,
//! then proxies inbound public requests to a local address.

use std::net::SocketAddr;

use futures::StreamExt;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio_yamux::{Config as YamuxConfig, Session};

use crate::protocol::{read_msg, write_msg, Hello, Reply};
use crate::record::{now_ms, Recorder, RequestRecord};
use crate::util::{text, Body};

/// What to share and where.
#[derive(Debug, Clone)]
pub struct ShareConfig {
    /// Tunnel server control address, e.g. `tunnel.example.com:7000`.
    pub server: String,
    /// Shared secret.
    pub token: String,
    /// Requested subdomain (server may override).
    pub subdomain: Option<String>,
    /// `Host` header the local site expects, e.g. `elyra-web.test`.
    pub local_host: String,
    /// Local address to proxy to, e.g. `127.0.0.1:80`.
    pub local_addr: SocketAddr,
    /// Optional `user:pass` Basic auth to enforce on the public URL.
    pub basic_auth: Option<String>,
}

/// Connect, register the tunnel, then serve until the connection drops.
///
/// `on_ready` is called once with the assigned public URL.
pub async fn run<F>(cfg: ShareConfig, recorder: Option<Recorder>, on_ready: F) -> anyhow::Result<()>
where
    F: FnOnce(&str, &str),
{
    let tcp = TcpStream::connect(&cfg.server).await?;
    let _ = tcp.set_nodelay(true);
    let mut session = Session::new_client(tcp, YamuxConfig::default());
    let mut control = session.control();

    // Drive the session: accept inbound (server-initiated) request streams and
    // proxy each to the local site.
    let local_addr = cfg.local_addr;
    let local_host = cfg.local_host.clone();
    let driver = tokio::spawn(async move {
        while let Some(item) = session.next().await {
            match item {
                Ok(stream) => {
                    let la = local_addr;
                    let lh = local_host.clone();
                    let rec = recorder.clone();
                    tokio::spawn(async move {
                        let svc =
                            service_fn(move |req| proxy_local(req, la, lh.clone(), rec.clone()));
                        if let Err(e) = http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), svc)
                            .await
                        {
                            tracing::debug!(error = %e, "tunnel stream ended");
                        }
                    });
                }
                Err(_) => break,
            }
        }
    });

    // Handshake over the control stream (the driver is already running).
    let mut ctrl = control.open_stream().await?;
    write_msg(
        &mut ctrl,
        &Hello {
            token: cfg.token.clone(),
            subdomain: cfg.subdomain.clone(),
            local_host: cfg.local_host.clone(),
            basic_auth: cfg.basic_auth.clone(),
        },
    )
    .await?;

    match read_msg::<_, Reply>(&mut ctrl).await? {
        Reply::Welcome {
            public_host,
            public_url,
        } => on_ready(&public_host, &public_url),
        Reply::Error { message } => {
            driver.abort();
            anyhow::bail!("server rejected tunnel: {message}");
        }
    }

    // Block until the server closes the control stream (EOF) or it errors.
    let mut buf = [0u8; 64];
    loop {
        match ctrl.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    driver.abort();
    Ok(())
}

/// Proxy a single tunnelled request to the local site.
async fn proxy_local(
    mut req: Request<Incoming>,
    addr: SocketAddr,
    local_host: String,
    recorder: Option<Recorder>,
) -> Result<Response<Body>, hyper::Error> {
    let started = std::time::Instant::now();
    let at_unix_ms = now_ms();
    let method = req.method().as_str().to_string();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let emit = move |status: u16| {
        if let Some(rec) = &recorder {
            rec(RequestRecord {
                at_unix_ms,
                method: method.clone(),
                path: path.clone(),
                status,
                duration_ms: started.elapsed().as_millis() as u64,
            });
        }
    };

    // Route to the local site without rewriting Host (so the app keeps seeing
    // the public hostname and builds correct asset URLs).
    if let Ok(hv) = hyper::header::HeaderValue::from_str(&local_host) {
        req.headers_mut().insert("x-grove-site", hv);
    }

    let tcp = match TcpStream::connect(addr).await {
        Ok(t) => t,
        Err(_) => {
            emit(0);
            return Ok(text(StatusCode::BAD_GATEWAY, "Local site unreachable.\n"));
        }
    };
    let (mut sender, conn) =
        match hyper::client::conn::http1::handshake::<_, Incoming>(TokioIo::new(tcp)).await {
            Ok(x) => x,
            Err(_) => {
                emit(0);
                return Ok(text(StatusCode::BAD_GATEWAY, "Local handshake failed.\n"));
            }
        };
    tokio::spawn(async move {
        let _ = conn.await;
    });

    match sender.send_request(req).await {
        Ok(resp) => {
            emit(resp.status().as_u16());
            Ok(resp.map(|b| b.boxed()))
        }
        Err(_) => {
            emit(0);
            Ok(text(StatusCode::BAD_GATEWAY, "Local site error.\n"))
        }
    }
}
