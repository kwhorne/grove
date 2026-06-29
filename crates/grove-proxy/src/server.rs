//! HTTP + HTTPS listeners that bind 80/443 and feed requests to the handler.

use std::net::SocketAddr;
use std::sync::Arc;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

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
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                continue;
            }
        };
        let state = state.clone();
        let fpm = fpm.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |req| {
                handler::handle(req, state.clone(), fpm.clone(), false, peer)
            });
            if let Err(e) = http1::Builder::new()
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
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let state = state.clone();
        let fpm = fpm.clone();
        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(error = %e, "TLS handshake failed");
                    return;
                }
            };
            let io = TokioIo::new(tls_stream);
            let service = service_fn(move |req| {
                handler::handle(req, state.clone(), fpm.clone(), true, peer)
            });
            if let Err(e) = http1::Builder::new()
                .serve_connection(io, service)
                .with_upgrades()
                .await
            {
                tracing::debug!(error = %e, "https connection closed");
            }
        });
    }
}
