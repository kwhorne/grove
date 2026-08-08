//! HTTP + HTTPS listeners that bind 80/443 and feed requests to the handler.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

/// How long a connection may stay silent before its request line and headers
/// must have arrived.
///
/// Without it an idle connection holds a task and a file descriptor for as long
/// as the peer likes, which is how a handful of half-open connections — a
/// crashed browser, a port scanner, `nc` left open — exhaust the descriptor
/// limit and take every site down with them.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// A TLS handshake must complete inside this. Same reasoning as
/// [`HEADER_READ_TIMEOUT`], but the handshake happens *before* hyper is
/// involved, so hyper's own timeout cannot cover it.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Accept one connection, absorbing transient errors.
///
/// A bare `continue` on error is a busy loop: `accept` fails immediately and
/// forever while the process is out of file descriptors (`EMFILE`), so the
/// listener spins a core at 100% and never recovers. Backing off leaves room for
/// descriptors to be released and keeps the daemon responsive meanwhile.
async fn accept(listener: &TcpListener) -> (TcpStream, SocketAddr) {
    let mut backoff = Duration::from_millis(5);
    loop {
        match listener.accept().await {
            Ok(pair) => return pair,
            Err(e) => {
                tracing::warn!(error = %e, backoff_ms = backoff.as_millis(), "accept failed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(1));
            }
        }
    }
}

/// The HTTP/1 server settings shared by both listeners.
fn http1_builder() -> http1::Builder {
    let mut builder = http1::Builder::new();
    builder.header_read_timeout(HEADER_READ_TIMEOUT);
    builder
}

use crate::handler::{self, FpmLocator};
use crate::state::SharedState;
use crate::tls::SniResolver;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("io binding {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}

/// Serve plain HTTP on `addr`.
pub async fn serve_http(
    addr: SocketAddr,
    state: SharedState,
    fpm: Arc<dyn FpmLocator>,
) -> Result<(), ServerError> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| ServerError::Bind { addr, source })?;
    tracing::info!(%addr, "HTTP listener bound");

    loop {
        let (stream, peer) = accept(&listener).await;
        // Interactive local traffic is latency-bound and mostly small writes
        // (a FastCGI flush, an SSE frame), so Nagle only adds delay.
        let _ = stream.set_nodelay(true);
        let state = state.clone();
        let fpm = fpm.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |req| {
                handler::handle(req, state.clone(), fpm.clone(), false, peer)
            });
            if let Err(e) = http1_builder()
                .serve_connection(io, service)
                .with_upgrades()
                .await
            {
                tracing::debug!(error = %e, "http connection closed");
            }
        });
    }
}

/// Serve HTTPS on `addr` using SNI to pick the correct per-site leaf cert.
pub async fn serve_https(
    addr: SocketAddr,
    state: SharedState,
    fpm: Arc<dyn FpmLocator>,
    sni: Arc<SniResolver>,
) -> Result<(), ServerError> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| ServerError::Bind { addr, source })?;

    let mut server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(sni);
    server_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    tracing::info!(%addr, "HTTPS listener bound");

    loop {
        let (stream, peer) = accept(&listener).await;
        let _ = stream.set_nodelay(true);
        let acceptor = acceptor.clone();
        let state = state.clone();
        let fpm = fpm.clone();
        tokio::spawn(async move {
            let tls_stream =
                match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => {
                        tracing::debug!(error = %e, "TLS handshake failed");
                        return;
                    }
                    Err(_) => {
                        tracing::debug!(%peer, "TLS handshake timed out");
                        return;
                    }
                };
            let io = TokioIo::new(tls_stream);
            let service =
                service_fn(move |req| handler::handle(req, state.clone(), fpm.clone(), true, peer));
            if let Err(e) = http1_builder()
                .serve_connection(io, service)
                .with_upgrades()
                .await
            {
                tracing::debug!(error = %e, "https connection closed");
            }
        });
    }
}
