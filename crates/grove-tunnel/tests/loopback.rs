//! End-to-end loopback test: a public request hits the tunnel server, is
//! relayed over yamux to the client, and reaches a local HTTP server — with the
//! Host header rewritten to the local site name.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header::HOST;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use grove_tunnel::client::{run as share, ShareConfig};
use grove_tunnel::server::{run as serve, ServerConfig};

/// Spawn a local HTTP server that echoes the Host header it received.
async fn spawn_local() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = service_fn(|req: Request<hyper::body::Incoming>| async move {
                    let host = req
                        .headers()
                        .get(HOST)
                        .and_then(|h| h.to_str().ok())
                        .unwrap_or("?")
                        .to_string();
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(format!(
                        "local-ok host={host}"
                    )))))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    addr
}

/// Issue a raw HTTP/1.1 GET with a chosen Host header and return the body.
async fn raw_get(addr: SocketAddr, host: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
async fn request_flows_through_tunnel() {
    let local = spawn_local().await;

    // Unique-ish high ports for the tunnel server.
    let control: SocketAddr = "127.0.0.1:39871".parse().unwrap();
    let http: SocketAddr = "127.0.0.1:39872".parse().unwrap();

    tokio::spawn(serve(ServerConfig {
        control_addr: control,
        http_addr: http,
        domain: "tun.local".into(),
        token: "secret".into(),
        scheme: "http".into(),
    }));

    // Wait for the server to bind.
    for _ in 0..50 {
        if TcpStream::connect(control).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let mut tx = Some(tx);
        share(
            ShareConfig {
                server: control.to_string(),
                token: "secret".into(),
                subdomain: Some("demo".into()),
                local_host: "local.test".into(),
                local_addr: local,
                basic_auth: None,
            },
            None,
            move |host, url| {
                if let Some(tx) = tx.take() {
                    let _ = tx.send((host.to_string(), url.to_string()));
                }
            },
        )
        .await
        .unwrap();
    });

    let (public_host, public_url) = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .expect("client should register in time")
        .expect("on_ready fired");
    assert_eq!(public_host, "demo.tun.local");
    assert_eq!(public_url, "http://demo.tun.local");

    // Give the registry a beat to settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = raw_get(http, "demo.tun.local").await;
    assert!(resp.contains("200 OK"), "expected 200, got:\n{resp}");
    assert!(resp.contains("local-ok"), "missing local body:\n{resp}");
    // The server must rewrite Host to the local site name.
    assert!(
        resp.contains("host=local.test"),
        "Host not rewritten:\n{resp}"
    );

    // Unknown subdomain → 404.
    let missing = raw_get(http, "nope.tun.local").await;
    assert!(missing.contains("404"), "expected 404, got:\n{missing}");
}
