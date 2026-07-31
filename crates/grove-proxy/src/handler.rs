//! Per-request dispatch: map a `Host` header to a site and serve it according
//! to its driver.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::{Body as _, Incoming};
use hyper::{Request, Response, StatusCode};

use grove_core::driver::Driver;
use grove_core::reqlog::{self, CapturedRequest, Record};
use grove_core::site::ResolvedSite;

use crate::fastcgi::{self, FpmAddr};
use crate::state::SharedState;

/// A response body that may be either one complete buffer or a live stream.
///
/// `Full<Bytes>` cannot represent a stream, so every response had to be fully
/// buffered before a single byte reached the client — which made Server-Sent
/// Events arrive all at once when PHP closed the request, and held a large
/// download entirely in memory.
type BoxBody = http_body_util::combinators::BoxBody<Bytes, std::io::Error>;

/// Wrap a complete buffer as a [`BoxBody`].
fn full(bytes: impl Into<Bytes>) -> BoxBody {
    Full::new(bytes.into())
        .map_err(|never| match never {})
        .boxed()
}

/// Captures the leading bytes of a request body for the timeline while the rest
/// streams through untouched.
///
/// The timeline already caps what it stores at [`reqlog::MAX_BODY`] and flags the
/// entry as truncated, so keeping only a prefix is the behaviour it expects — it
/// just used to arrive by buffering everything first and cutting afterwards.
#[derive(Clone, Default)]
struct BodyTap(Arc<std::sync::Mutex<Vec<u8>>>);

impl BodyTap {
    fn push(&self, chunk: &[u8]) {
        let Ok(mut buf) = self.0.lock() else { return };
        let room = reqlog::MAX_BODY.saturating_sub(buf.len());
        if room > 0 {
            buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
    }

    fn take(&self) -> Vec<u8> {
        self.0.lock().map(|b| b.clone()).unwrap_or_default()
    }
}

/// Pass a request body through, copying its first bytes into `tap`.
fn tapped_body(body: Incoming, tap: BodyTap) -> BoxBody {
    use futures::StreamExt;

    let stream = futures::stream::unfold(body, |mut body| async move {
        body.frame().await.map(|frame| (frame, body))
    })
    .map(move |frame| {
        frame
            .inspect(|frame| {
                if let Some(data) = frame.data_ref() {
                    tap.push(data);
                }
            })
            .map_err(std::io::Error::other)
    });

    BodyExt::boxed(http_body_util::StreamBody::new(stream))
}

/// Wrap a stream of chunks as a [`BoxBody`], one HTTP chunk per item.
fn streaming(rx: fastcgi::BodyStream) -> BoxBody {
    use futures::StreamExt;

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|chunk| (chunk, rx))
    })
    .map(|chunk| chunk.map(hyper::body::Frame::data));

    // Disambiguated: `boxed` exists on both BodyExt and StreamExt.
    BodyExt::boxed(http_body_util::StreamBody::new(stream))
}

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
    let start = std::time::Instant::now();
    let method = req.method().as_str().to_string();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());

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

    // Capture headers + body for the timeline and replay before the body is
    // consumed downstream. One buffering clone; bodies are typically tiny and
    // capped in the log.
    let req_headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
        .collect();
    let (parts, body) = req.into_parts();

    // Bodies the timeline could store in full are still collected, so `replay`
    // and the curl/.http/Pest export are unchanged for what is almost every
    // request. Only uploads larger than the timeline's own cap stream through —
    // where a truncated capture is already what the timeline would have kept.
    let exact = body.size_hint().exact();
    let streams = exact.is_some_and(|n| n > reqlog::MAX_BODY as u64);

    let (tap, body_len, forwarded) = if streams {
        let tap = BodyTap::default();
        let len = exact.unwrap_or_default();
        (tap.clone(), len, tapped_body(body, tap))
    } else {
        let bytes = body
            .collect()
            .await
            .map(|b| b.to_bytes())
            .unwrap_or_default();
        let len = bytes.len() as u64;
        let tap = BodyTap::default();
        tap.push(&bytes);
        (tap, len, full(bytes))
    };
    let req = Request::from_parts(parts, forwarded);

    // Webhook capture: any request to `/__grove/hooks[/bucket]` is recorded and
    // acknowledged with 200 (never dispatched to the app). Expose it publicly
    // with `grove share <site>` and point Stripe/GitHub at it.
    const HOOK_PREFIX: &str = "/__grove/hooks";
    let just_path = path.split('?').next().unwrap_or(path.as_str());
    if just_path == HOOK_PREFIX || just_path.starts_with(&format!("{HOOK_PREFIX}/")) {
        let bucket = just_path
            .strip_prefix(HOOK_PREFIX)
            .map(|s| s.trim_start_matches('/'))
            .filter(|s| !s.is_empty())
            .unwrap_or("default");
        state.hooks.record(Record {
            site: bucket,
            host: &host,
            method: &method,
            path: &path,
            status: StatusCode::OK.as_u16(),
            duration_ms: 0,
            https,
            headers: req_headers,
            body: tap.take(),
        });
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(full(Bytes::from_static(b"{\"grove\":\"captured\"}")))
            .expect("static hook ack"));
    }

    let site = {
        let registry = state.registry.read().await;
        registry.by_hostname(&route_host).cloned()
    };

    let Some(site) = site else {
        state.log.record(Record {
            site: &route_host,
            host: &host,
            method: &method,
            path: &path,
            status: StatusCode::NOT_FOUND.as_u16(),
            duration_ms: start.elapsed().as_millis() as u64,
            https,
            headers: req_headers,
            body: tap.take(),
        });
        return Ok(text_response(
            StatusCode::NOT_FOUND,
            &format!("Grove: no site registered for host {route_host:?}"),
        ));
    };

    if site.docker && !site.docker_running {
        state.log.record(Record {
            site: &site.name,
            host: &host,
            method: &method,
            path: &path,
            status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
            duration_ms: start.elapsed().as_millis() as u64,
            https,
            headers: req_headers,
            body: tap.take(),
        });
        return Ok(text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!(
                "Grove: the container for {} is stopped — start it from the Sites list.",
                site.hostname
            ),
        ));
    }

    tracing::debug!(host, site = %site.name, driver = %site.driver, %peer, "dispatch");

    let result = if is_hidden_path(&sanitize_path(req.uri().path())) {
        // `.env`, `.git/config`, editor backups: never served, never executed.
        Ok(text_response(StatusCode::NOT_FOUND, "Grove: not found"))
    } else {
        match site.driver {
            Driver::Proxy => serve_proxy(req, &site, body_len).await,
            Driver::Static => serve_static(req, &site).await,
            d if d.is_php() => {
                // try_files: serve an existing static file (e.g. built Vite assets
                // under /build/) directly, otherwise hand off to the front
                // controller (index.php).
                let rel = sanitize_path(req.uri().path());
                let candidate = site.document_root.join(&rel);
                if !rel.as_os_str().is_empty() && candidate.is_file() {
                    if is_php_script(&rel) {
                        // Execute it, never serve it. Reading it out as text would
                        // disclose source (`/index.php` leaked the front controller)
                        // and would break any app that addresses scripts directly,
                        // such as WordPress's wp-login.php and wp-admin/*.php.
                        serve_php(req, &site, fpm.as_ref(), https, Some(rel), body_len).await
                    } else {
                        serve_static(req, &site).await
                    }
                } else {
                    serve_php(req, &site, fpm.as_ref(), https, None, body_len).await
                }
            }
            _ => serve_static(req, &site).await,
        }
    };

    let response = result.unwrap_or_else(|e| {
        tracing::error!(error = %e, site = %site.name, "request failed");
        text_response(StatusCode::BAD_GATEWAY, &format!("Grove: {e}"))
    });
    state.log.record(Record {
        site: &site.name,
        host: &host,
        method: &method,
        path: &path,
        status: response.status().as_u16(),
        duration_ms: start.elapsed().as_millis() as u64,
        https,
        headers: req_headers,
        body: tap.take(),
    });
    Ok(response)
}

/// Re-issue a captured request through Grove's own HTTP port so it flows through
/// the full proxy pipeline again (and is logged as a fresh entry). Routing is by
/// the original `Host` header. Returns `(status, duration_ms)`.
pub async fn replay(http_port: u16, cap: &CapturedRequest) -> anyhow::Result<(u16, u64)> {
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;

    let uri: hyper::Uri = format!("http://127.0.0.1:{}{}", http_port, cap.path).parse()?;
    let mut builder = Request::builder().method(cap.method.as_str()).uri(uri);
    for (k, v) in &cap.headers {
        match k.to_ascii_lowercase().as_str() {
            "host" | "content-length" | "connection" | "transfer-encoding" => continue,
            _ => builder = builder.header(k, v),
        }
    }
    builder = builder.header(hyper::header::HOST, &cap.host);
    if cap.https {
        builder = builder.header("x-forwarded-proto", "https");
    }
    let request = builder.body(Full::new(Bytes::from(cap.body.clone())))?;

    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let start = std::time::Instant::now();
    let resp = client.request(request).await?;
    Ok((resp.status().as_u16(), start.elapsed().as_millis() as u64))
}

/// Replay a captured request to a specific local target (host + path), routed
/// through Grove's HTTP port — used to re-deliver a captured webhook to your
/// app's real handler. Returns `(status, duration_ms)`.
pub async fn replay_to(
    http_port: u16,
    cap: &CapturedRequest,
    target_host: &str,
    target_path: &str,
    https: bool,
) -> anyhow::Result<(u16, u64)> {
    let mut c = cap.clone();
    c.host = target_host.to_string();
    c.path = target_path.to_string();
    c.https = https;
    replay(http_port, &c).await
}

/// Serve a static file from the document root, with a directory-index fallback.
async fn serve_static(
    req: Request<BoxBody>,
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
        .body(full(bytes))?;
    Ok(resp)
}

/// Dispatch a request to PHP-FPM over FastCGI.
/// True for paths containing a dot-prefixed component, which must never be
/// served or executed.
///
/// A plain PHP project's document root *is* the project root, so `/.env` handed
/// back `APP_KEY` in full — and publicly, for a site exposed with `grove share`.
///
/// `.well-known/` is deliberately exempt: it is a standard public path, and
/// blocking it would break ACME HTTP-01 challenges.
fn is_hidden_path(rel: &Path) -> bool {
    if rel
        .components()
        .next()
        .is_some_and(|first| first.as_os_str() == ".well-known")
    {
        return false;
    }
    rel.components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

/// True for paths PHP-FPM must execute rather than Grove serve as bytes.
///
/// Compared case-insensitively: macOS filesystems are case-insensitive by
/// default, so `/INDEX.PHP` resolves to the same file and must not slip through
/// as a static download.
fn is_php_script(rel: &Path) -> bool {
    rel.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            e == "php" || e == "phtml"
        })
        .unwrap_or(false)
}

/// Serve a request through PHP-FPM.
///
/// `direct` names a document-root-relative script to execute on its own;
/// `None` routes the request through the site's front controller.
async fn serve_php(
    req: Request<BoxBody>,
    site: &ResolvedSite,
    fpm: &dyn FpmLocator,
    https: bool,
    direct: Option<PathBuf>,
    body_len: u64,
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
    // A directly addressed script is its own SCRIPT_NAME; the front controller
    // additionally receives the request path as PATH_INFO, which is how a router
    // sees the URL it should dispatch.
    let (script_name, path_info) = match &direct {
        Some(rel) => (rel.clone(), false),
        None => (front, true),
    };
    let script = site.document_root.join(&script_name);

    let (parts, body) = req.into_parts();

    // CONTENT_LENGTH must match the bytes actually streamed into STDIN: too high
    // and PHP-FPM waits forever for bytes that never come, too low and the body
    // is silently truncated.
    let params = build_fcgi_params(
        &parts,
        site,
        &script,
        &script_name,
        path_info,
        body_len,
        https,
    );
    // Streaming: the headers come back as soon as PHP flushes them, and the body
    // follows as chunks. stderr is logged by the FastCGI layer, since once the
    // headers are on the wire it can no longer become a 500.
    let (headers, body_rx) = fastcgi::request_streaming(&addr, &params, body).await?;

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
    // PHP sets no Content-Length on a streamed response, so hyper picks
    // Transfer-Encoding: chunked by itself.
    let resp = builder.status(status).body(streaming(body_rx))?;
    Ok(resp)
}

/// Forward to an upstream dev server (Vite/Node) proxy driver.
async fn serve_proxy(
    req: Request<BoxBody>,
    site: &ResolvedSite,
    body_len: u64,
) -> Result<Response<BoxBody>, anyhow::Error> {
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;

    let Some(upstream) = &site.proxy_to else {
        return Ok(text_response(
            StatusCode::BAD_GATEWAY,
            "Grove: proxy site has no upstream configured",
        ));
    };

    // The upstream sees the body as a stream; the length is only of interest to
    // the FastCGI path.
    let _ = body_len;
    let path_q = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let uri: hyper::Uri = format!("{}{}", upstream.trim_end_matches('/'), path_q).parse()?;

    let (mut parts, body) = req.into_parts();
    // Preserve the public host for apps that honour X-Forwarded-Host, but set
    // Host to the upstream authority so name-based vhosts (nginx / OrbStack
    // `server_name`) match instead of falling to a default server block.
    let orig_host = parts
        .headers
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    parts.uri = uri;
    if let Some(auth) = parts.uri.authority().map(|a| a.as_str().to_string()) {
        if let Ok(hv) = hyper::header::HeaderValue::from_str(&auth) {
            parts.headers.insert(hyper::header::HOST, hv);
        }
    }
    if let Some(oh) = orig_host {
        if let Ok(hv) = hyper::header::HeaderValue::from_str(&oh) {
            parts.headers.insert("x-forwarded-host", hv);
        }
    }
    parts.headers.insert(
        "x-forwarded-proto",
        hyper::header::HeaderValue::from_static("https"),
    );
    // Nothing here needs the length up front — CGI's CONTENT_LENGTH requirement
    // does not apply to an HTTP upstream — so the body is forwarded as it
    // arrives, whatever its transfer encoding.
    let forwarded = Request::from_parts(parts, body);

    let client: Client<_, BoxBody> = Client::builder(TokioExecutor::new()).build_http();
    let resp = client.request(forwarded).await?;
    let (parts, body) = resp.into_parts();
    // Pass the upstream body through without collecting it, so a Vite HMR stream
    // or a Node SSE endpoint reaches the client as it arrives.
    let body = body.map_err(std::io::Error::other).boxed();
    Ok(Response::from_parts(parts, body))
}

/// Build the CGI/1.1 environment FastCGI expects.
fn build_fcgi_params(
    parts: &hyper::http::request::Parts,
    site: &ResolvedSite,
    script: &Path,
    script_name: &Path,
    path_info: bool,
    content_length: u64,
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
    p.insert("SCRIPT_NAME".into(), format!("/{}", script_name.display()));
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
    // Only meaningful for a front controller. Setting it for a directly executed
    // script would claim the script's own path is extra path information.
    if path_info {
        p.insert("PATH_INFO".into(), path.clone());
    }
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
    p.insert("CONTENT_LENGTH".into(), content_length.to_string());

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
        .body(full(msg.to_string()))
        .expect("static response builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_paths_are_refused() {
        // Regression: a plain PHP project's document root is the project root, so
        // these returned real secrets in full.
        assert!(is_hidden_path(Path::new(".env")));
        assert!(is_hidden_path(Path::new(".env.production")));
        assert!(is_hidden_path(Path::new(".git/config")));
        assert!(is_hidden_path(Path::new("storage/.env.backup")));
        assert!(is_hidden_path(Path::new("nested/.ssh/id_rsa")));
    }

    #[test]
    fn well_known_stays_reachable() {
        // ACME HTTP-01 would break otherwise.
        assert!(!is_hidden_path(Path::new(
            ".well-known/acme-challenge/token"
        )));
        assert!(!is_hidden_path(Path::new(".well-known/security.txt")));
        // Only as the *first* component, so this is still hidden.
        assert!(is_hidden_path(Path::new("public/.well-known/x")));
    }

    #[test]
    fn ordinary_paths_are_unaffected() {
        assert!(!is_hidden_path(Path::new("")));
        assert!(!is_hidden_path(Path::new("index.php")));
        assert!(!is_hidden_path(Path::new("build/app.css")));
        assert!(!is_hidden_path(Path::new("img/logo.svg")));
    }

    #[test]
    fn php_scripts_are_executed_not_served() {
        // Regression: `/index.php` used to be served as text, disclosing the
        // front controller's source on every PHP site — and reachable publicly
        // through `grove share`.
        assert!(is_php_script(Path::new("index.php")));
        assert!(is_php_script(Path::new("wp-login.php")));
        assert!(is_php_script(Path::new("wp-admin/admin-ajax.php")));
        assert!(is_php_script(Path::new("legacy/install.phtml")));
        // Case-insensitive filesystems resolve these to the same file.
        assert!(is_php_script(Path::new("INDEX.PHP")));
        assert!(is_php_script(Path::new("Index.Php")));
    }

    #[test]
    fn assets_are_still_served_statically() {
        assert!(!is_php_script(Path::new("build/app.css")));
        assert!(!is_php_script(Path::new("img/logo.svg")));
        assert!(!is_php_script(Path::new("favicon.ico")));
        // No extension at all, e.g. a pretty URL that happens to exist as a file.
        assert!(!is_php_script(Path::new("robots")));
        // Not an executable script: the extension is what matters, and these
        // must not be handed to FPM.
        assert!(!is_php_script(Path::new("archive.phar")));
        assert!(!is_php_script(Path::new("notes.php.txt")));
    }

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(
            sanitize_path("/../../etc/passwd"),
            PathBuf::from("etc/passwd")
        );
        assert_eq!(sanitize_path("/css/app.css"), PathBuf::from("css/app.css"));
    }
}
