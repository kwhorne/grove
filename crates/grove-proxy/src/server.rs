//! HTTP + HTTPS listeners that bind 80/443 and feed requests to the handler.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
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

/// One connection builder for both listeners: HTTP/1.1 and HTTP/2, chosen per
/// connection — by ALPN on the TLS listener, by preface sniffing on the plain
/// one.
///
/// HTTP/2 was off for a long time: ALPN offered only `http/1.1`, so every
/// browser fell back to six connections per origin, and a Laravel page with
/// forty Vite module requests or a WordPress admin screen loaded them in
/// batches.
///
/// The timers are not optional, for either protocol. hyper does not fall back
/// to a default one, and it does not fail when a timeout is configured — it
/// panics on the first connection that reaches the timeout code, which is every
/// connection: "timeout `header_read_timeout` set, but no timer set". Because
/// panics unwind, that took down every site while the daemon stayed up and
/// reported itself healthy. The tests below put a request through each protocol
/// on a connection built exactly this way.
pub(crate) fn connection_builder() -> auto::Builder<TokioExecutor> {
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
    builder.http2().timer(TokioTimer::new());
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
    let listener = bind(addr).await?;
    serve_http_on(listener, state, fpm).await
}

/// Bind `addr`, so a caller can learn whether the port was actually taken
/// before the accept loop begins. `serve_http`/`serve_https` bind and serve in
/// one step; the daemon binds first, records the result for `grove status`, and
/// then hands the listener to `serve_http_on`/`serve_https_on`. Without that
/// split a failed bind was one log line, and the daemon went on reporting a
/// listener it did not have.
pub async fn bind(addr: SocketAddr) -> Result<TcpListener, ServerError> {
    TcpListener::bind(addr)
        .await
        .map_err(|source| ServerError::Bind { addr, source })
}

/// Serve plain HTTP on an already-bound listener.
pub async fn serve_http_on(
    listener: TcpListener,
    state: SharedState,
    fpm: Arc<dyn FpmLocator>,
) -> Result<(), ServerError> {
    if let Ok(addr) = listener.local_addr() {
        tracing::info!(%addr, "HTTP listener bound");
    }

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
            if let Err(e) = connection_builder()
                .serve_connection_with_upgrades(io, service)
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
    let listener = bind(addr).await?;
    serve_https_on(listener, state, fpm, sni).await
}

/// Serve HTTPS on an already-bound listener; see [`bind`].
pub async fn serve_https_on(
    listener: TcpListener,
    state: SharedState,
    fpm: Arc<dyn FpmLocator>,
    sni: Arc<SniResolver>,
) -> Result<(), ServerError> {
    let mut server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(sni);
    // h2 first; http/1.1 stays for clients that cannot negotiate it.
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    if let Ok(addr) = listener.local_addr() {
        tracing::info!(%addr, "HTTPS listener bound");
    }

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
            if let Err(e) = connection_builder()
                .serve_connection_with_upgrades(io, service)
                .await
            {
                tracing::debug!(error = %e, "https connection closed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty, Full};
    use hyper::{Request, Response};

    /// Serve exactly one connection with the listeners' own settings and read
    /// the response back.
    ///
    /// Regression: `header_read_timeout` was configured without a timer.
    /// hyper does not fall back to a default and does not fail at setup — it
    /// panics on the first connection that reaches the timeout code, which is
    /// every connection ("timeout `header_read_timeout` set, but no timer set").
    /// Every site on the machine answered with a reset connection, and because
    /// panics unwind rather than abort, the daemon stayed up and reported
    /// itself healthy while serving nothing.
    ///
    /// Every layer had been tested on its own; nothing had put a request
    /// through a connection built the way the listeners build it. This does.
    #[tokio::test]
    async fn a_connection_built_like_the_listeners_serves_a_request() {
        let (client, server) = tokio::io::duplex(4096);

        tokio::spawn(async move {
            let service =
                hyper::service::service_fn(|_req: Request<hyper::body::Incoming>| async {
                    Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(
                        b"served",
                    ))))
                });
            let _ = connection_builder()
                .serve_connection_with_upgrades(TokioIo::new(server), service)
                .await;
        });

        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(client))
            .await
            .expect("handshake");
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let req = Request::builder()
            .uri("/")
            .header(hyper::header::HOST, "probe.test")
            .body(Empty::<Bytes>::new())
            .expect("request builds");

        let resp = sender.send_request(req).await.expect("a response arrives");
        assert_eq!(resp.status(), hyper::StatusCode::OK);
        let body = resp.into_body().collect().await.expect("body").to_bytes();
        assert_eq!(&body[..], b"served");
    }

    /// The same builder, spoken as HTTP/2. The plain listener has no ALPN, so
    /// this exercises preface sniffing; the TLS listener reaches the same
    /// builder once ALPN has picked `h2`. A missing http2 timer would surface
    /// here the way the missing http1 one did above.
    #[tokio::test]
    async fn the_same_builder_serves_http2() {
        let (client, server) = tokio::io::duplex(8192);

        tokio::spawn(async move {
            let service =
                hyper::service::service_fn(|req: Request<hyper::body::Incoming>| async move {
                    assert_eq!(req.version(), hyper::Version::HTTP_2);
                    // What h2 hands a handler: no Host header, authority on the
                    // URI. `handler::handle` must read the latter or every site
                    // 404s the moment a browser negotiates h2.
                    assert!(req.headers().get(hyper::header::HOST).is_none());
                    assert_eq!(
                        req.uri().authority().map(|a| a.as_str()),
                        Some("probe.test")
                    );
                    Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(
                        b"served over h2",
                    ))))
                });
            let _ = connection_builder()
                .serve_connection_with_upgrades(TokioIo::new(server), service)
                .await;
        });

        let (mut sender, conn) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(client))
                .await
                .expect("h2 handshake");
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let req = Request::builder()
            .uri("https://probe.test/")
            .body(Empty::<Bytes>::new())
            .expect("request builds");
        let resp = sender.send_request(req).await.expect("a response arrives");
        assert_eq!(resp.version(), hyper::Version::HTTP_2);
        assert_eq!(resp.status(), hyper::StatusCode::OK);
        let body = resp.into_body().collect().await.expect("body").to_bytes();
        assert_eq!(&body[..], b"served over h2");
    }
}
