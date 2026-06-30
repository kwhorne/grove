//! Per-request dispatch: map a `Host` header to a site and serve it according
//! to its driver.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};

use grove_core::driver::Driver;
use grove_core::site::ResolvedSite;

use crate::fastcgi::{self, FpmAddr};
use crate::state::SharedState;

type BoxBody = Full<Bytes>;

/// Locate the FastCGI pool for a given PHP version. Implemented by grove-runtime.
pub trait FpmLocator: Send + Sync {
    fn locate(&self, php_version: &str) -> Option<FpmAddr>;
}

/// Handle one incoming request end to end. Never panics — every error path
/// becomes an HTTP status so one bad site can't take down the daemon.
pub async fn handle(
    req: Request<Incoming>,
    state: SharedState,
    fpm: Arc<dyn FpmLocator>,
    https: bool,
    peer: SocketAddr,
) -> Result<Response<BoxBody>, Infallible> {
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_default();

    // A tunnel keeps the public Host (so the app builds correct asset URLs) and
    // carries the local site name in X-Grove-Site purely for routing.
    let route_host = req
        .headers()
        .get("x-grove-site")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| host.clone());

    // Honour X-Forwarded-Proto (set by the tunnel) so generated URLs use https.
    let https = https
        || req
            .headers()
            .get("x-forwarded-proto")
            .and_then(|h| h.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("https"))
            .unwrap_or(false);

    let site = {
        let registry = state.registry.read().await;
        registry.by_hostname(&route_host).cloned()
    };

    let Some(site) = site else {
        return Ok(text_response(
            StatusCode::NOT_FOUND,
            &format!("Grove: no site registered for host {route_host:?}"),
        ));
    };

    tracing::debug!(host, site = %site.name, driver = %site.driver, %peer, "dispatch");

    let result = match site.driver {
        Driver::Proxy => serve_proxy(req, &site).await,
        Driver::Static => serve_static(req, &site).await,
        d if d.is_php() => {
            // try_files: serve an existing static file (e.g. built Vite assets
            // under /build/) directly, otherwise hand off to the front
            // controller (index.php).
            let rel = sanitize_path(req.uri().path());
            let candidate = site.document_root.join(&rel);
            if !rel.as_os_str().is_empty() && candidate.is_file() {
                serve_static(req, &site).await
            } else {
                serve_php(req, &site, fpm.as_ref(), https).await
            }
        }
        _ => serve_static(req, &site).await,
    };

    Ok(result.unwrap_or_else(|e| {
        tracing::error!(error = %e, site = %site.name, "request failed");
        text_response(StatusCode::BAD_GATEWAY, &format!("Grove: {e}"))
    }))
}

/// Serve a static file from the document root, with a directory-index fallback.
async fn serve_static(
    req: Request<Incoming>,
    site: &ResolvedSite,
) -> Result<Response<BoxBody>, anyhow::Error> {
    let rel = sanitize_path(req.uri().path());
    let mut target = site.document_root.join(&rel);

    if target.is_dir() {
        target = target.join("index.html");
    }
    if !target.exists() {
        // SPA-style fallback to a root index if present.
        let fallback = site.document_root.join("index.html");
        if fallback.exists() {
            target = fallback;
        } else {
            return Ok(text_response(
                StatusCode::NOT_FOUND,
                "Grove: file not found",
            ));
        }
    }

    let bytes = tokio::fs::read(&target).await?;
    let mime = mime_for(&target);
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, mime)
        .body(Full::new(Bytes::from(bytes)))?;
    Ok(resp)
}

/// Dispatch a request to PHP-FPM over FastCGI.
async fn serve_php(
    req: Request<Incoming>,
    site: &ResolvedSite,
    fpm: &dyn FpmLocator,
    https: bool,
) -> Result<Response<BoxBody>, anyhow::Error> {
    let Some(addr) = fpm.locate(&site.php) else {
        return Ok(text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("Grove: no PHP-FPM pool for php@{}", site.php),
        ));
    };

    let front = site
        .front_controller
        .clone()
        .unwrap_or_else(|| PathBuf::from("index.php"));
    let script = site.document_root.join(&front);

    let (parts, body) = req.into_parts();
    let body_bytes = body.collect().await?.to_bytes();

    let params = build_fcgi_params(&parts, site, &script, &front, &body_bytes, https);
    let resp = fastcgi::request(&addr, &params, &body_bytes).await?;

    if !resp.stderr.is_empty() {
        tracing::warn!(site = %site.name, stderr = %String::from_utf8_lossy(&resp.stderr));
    }

    let (headers, php_body) = fastcgi::split_headers(&resp.stdout);
    let mut builder = Response::builder();
    let mut status = StatusCode::OK;
    for (name, value) in &headers {
        if name.eq_ignore_ascii_case("Status") {
            if let Some(code) = value.split_whitespace().next().and_then(|c| c.parse().ok()) {
                status = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
            }
            continue;
        }
        builder = builder.header(name, value);
    }
    let resp = builder
        .status(status)
        .body(Full::new(Bytes::from(php_body)))?;
    Ok(resp)
}

/// Forward to an upstream dev server (Vite/Node) proxy driver.
async fn serve_proxy(
    req: Request<Incoming>,
    site: &ResolvedSite,
) -> Result<Response<BoxBody>, anyhow::Error> {
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;

    let Some(upstream) = &site.proxy_to else {
        return Ok(text_response(
            StatusCode::BAD_GATEWAY,
            "Grove: proxy site has no upstream configured",
        ));
    };

    let path_q = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let uri: hyper::Uri = format!("{}{}", upstream.trim_end_matches('/'), path_q).parse()?;

    let (mut parts, body) = req.into_parts();
    parts.uri = uri;
    let body_bytes = body.collect().await?.to_bytes();
    let forwarded = Request::from_parts(parts, Full::new(body_bytes));

    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let resp = client.request(forwarded).await?;
    let (parts, body) = resp.into_parts();
    let collected = body.collect().await?.to_bytes();
    Ok(Response::from_parts(parts, Full::new(collected)))
}

/// Build the CGI/1.1 environment FastCGI expects.
fn build_fcgi_params(
    parts: &hyper::http::request::Parts,
    site: &ResolvedSite,
    script: &Path,
    front: &Path,
    body: &[u8],
    https: bool,
) -> HashMap<String, String> {
    let mut p = HashMap::new();
    let uri = &parts.uri;
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();

    p.insert("GATEWAY_INTERFACE".into(), "CGI/1.1".into());
    p.insert("SERVER_SOFTWARE".into(), "Grove".into());
    p.insert("REQUEST_METHOD".into(), parts.method.to_string());
    p.insert("SCRIPT_FILENAME".into(), script.to_string_lossy().into());
    p.insert("SCRIPT_NAME".into(), format!("/{}", front.display()));
    p.insert(
        "DOCUMENT_ROOT".into(),
        site.document_root.to_string_lossy().into(),
    );
    p.insert("REQUEST_URI".into(), {
        if query.is_empty() {
            path.clone()
        } else {
            format!("{path}?{query}")
        }
    });
    p.insert("PATH_INFO".into(), path.clone());
    p.insert("QUERY_STRING".into(), query);
    p.insert("SERVER_NAME".into(), site.hostname.clone());
    p.insert(
        "SERVER_PORT".into(),
        if https { "443".into() } else { "80".into() },
    );
    p.insert("SERVER_PROTOCOL".into(), "HTTP/1.1".into());
    p.insert(
        "HTTPS".into(),
        if https { "on".into() } else { String::new() },
    );
    p.insert(
        "REQUEST_SCHEME".into(),
        if https { "https".into() } else { "http".into() },
    );
    p.insert("CONTENT_LENGTH".into(), body.len().to_string());

    if let Some(ct) = parts.headers.get(hyper::header::CONTENT_TYPE) {
        if let Ok(v) = ct.to_str() {
            p.insert("CONTENT_TYPE".into(), v.to_string());
        }
    }

    // Forward all request headers as HTTP_* CGI variables.
    for (name, value) in parts.headers.iter() {
        if let Ok(v) = value.to_str() {
            let key = format!("HTTP_{}", name.as_str().to_uppercase().replace('-', "_"));
            p.insert(key, v.to_string());
        }
    }

    p
}

fn sanitize_path(path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.trim_start_matches('/').split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn text_response(status: StatusCode, msg: &str) -> Response<BoxBody> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(msg.to_string())))
        .expect("static response builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(
            sanitize_path("/../../etc/passwd"),
            PathBuf::from("etc/passwd")
        );
        assert_eq!(sanitize_path("/css/app.css"), PathBuf::from("css/app.css"));
    }
}
