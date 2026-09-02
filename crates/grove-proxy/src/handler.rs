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

/// A body with no declared length is read into memory up to this much before it
/// spills to disk. Matches the timeline's own cap, so the bytes the timeline
/// keeps are exactly the bytes held in memory.
const SPOOL_THRESHOLD: usize = reqlog::MAX_BODY;

/// Hard ceiling on a single request body. Without one, an unbounded chunked
/// upload is a disk-filling vector, since CGI forces Grove to know the length
/// before it can forward anything.
const MAX_REQUEST_BODY: u64 = 2 * 1024 * 1024 * 1024;

/// A spooled body file, removed when the body that reads it is dropped.
///
/// Deletion is tied to `Drop` rather than to the end of the happy path, so an
/// error or a client disconnect cannot leave upload contents on disk.
struct SpoolFile(PathBuf);

impl Drop for SpoolFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A private, user-owned directory for spooled bodies.
///
/// Not the shared temp root directly: on Linux `/tmp` is world-readable and an
/// upload can contain anything. The directory is `0700` and the files `0600`.
fn spool_dir() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("grove-spool-{}", current_uid()));
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        extern "C" {
            fn geteuid() -> u32;
        }
        unsafe { geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Create a spool file that no other user can read.
async fn create_spool_file() -> std::io::Result<(tokio::fs::File, SpoolFile)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = spool_dir()?;
    let name = format!(
        "body-{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let path = dir.join(name);

    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        // tokio's OpenOptions exposes `mode` inherently on unix.
        opts.mode(0o600);
    }
    let file = opts.open(&path).await?;
    Ok((file, SpoolFile(path)))
}

/// The outcome of reading a body whose length was not declared.
enum Unsized {
    /// Small enough to keep in memory, so it behaves like any other body.
    Buffered(Bytes),
    /// Spilled to disk; `len` is the measured length for `CONTENT_LENGTH`.
    Spooled {
        file: SpoolFile,
        len: u64,
        prefix: Vec<u8>,
    },
    /// Beyond [`MAX_REQUEST_BODY`].
    TooLarge,
}

/// Read a body of unknown length, measuring it so CGI can be told the length.
///
/// Chunked requests do not declare a length, but `CONTENT_LENGTH` must be sent
/// before the body. Refusing them with `411` would make Grove the reason a valid
/// request fails — nginx and Apache spool instead — so Grove spools too, keeping
/// small bodies in memory and only touching disk when it must.
async fn read_unsized_body(mut body: Incoming) -> std::io::Result<Unsized> {
    use tokio::io::AsyncWriteExt;

    let mut buffered: Vec<u8> = Vec::new();
    let mut spool: Option<(tokio::fs::File, SpoolFile)> = None;
    let mut total: u64 = 0;

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(std::io::Error::other)?;
        let Some(data) = frame.data_ref() else {
            continue;
        };
        total += data.len() as u64;
        if total > MAX_REQUEST_BODY {
            return Ok(Unsized::TooLarge);
        }

        match &mut spool {
            Some((file, _)) => file.write_all(data).await?,
            None if buffered.len() + data.len() > SPOOL_THRESHOLD => {
                // Crossed the threshold: open the file and flush what we held.
                let (mut file, guard) = create_spool_file().await?;
                file.write_all(&buffered).await?;
                file.write_all(data).await?;
                spool = Some((file, guard));
            }
            None => buffered.extend_from_slice(data),
        }
    }

    match spool {
        None => Ok(Unsized::Buffered(Bytes::from(buffered))),
        Some((mut file, guard)) => {
            file.flush().await?;
            // The prefix is what the timeline keeps; it is already in memory.
            buffered.truncate(reqlog::MAX_BODY);
            Ok(Unsized::Spooled {
                file: guard,
                len: total,
                prefix: buffered,
            })
        }
    }
}

/// Stream a spooled body from disk, deleting the file when the body is dropped.
async fn spooled_body(file: SpoolFile) -> std::io::Result<BoxBody> {
    use futures::StreamExt;
    use tokio::io::AsyncReadExt;

    let handle = tokio::fs::File::open(&file.0).await?;
    let state = Some((handle, file, vec![0u8; 64 * 1024]));

    let stream = futures::stream::unfold(state, |state| async move {
        let (mut handle, guard, mut buf) = state?;
        match handle.read(&mut buf).await {
            // EOF: dropping `guard` here removes the file.
            Ok(0) => None,
            Ok(n) => {
                let chunk = Bytes::copy_from_slice(&buf[..n]);
                Some((Ok(chunk), Some((handle, guard, buf))))
            }
            Err(e) => Some((Err(e), None)),
        }
    })
    .map(|chunk| chunk.map(hyper::body::Frame::data));

    Ok(BodyExt::boxed(http_body_util::StreamBody::new(stream)))
}

/// Captures the leading bytes of a request body for the timeline while the rest
/// streams through untouched.
///
/// The timeline already caps what it stores at [`reqlog::MAX_BODY`] and flags the
/// entry as truncated, so keeping only a prefix is the behaviour it expects — it
/// just used to arrive by buffering everything first and cutting afterwards.
#[derive(Clone, Default)]
struct BodyTap(Arc<std::sync::Mutex<(Vec<u8>, bool)>>);

impl BodyTap {
    fn push(&self, chunk: &[u8]) {
        let Ok(mut state) = self.0.lock() else { return };
        let (buf, truncated) = &mut *state;
        let room = reqlog::MAX_BODY.saturating_sub(buf.len());
        if chunk.len() > room {
            // Remember that bytes were dropped. The timeline cannot infer it: the
            // capture stops at exactly MAX_BODY, which is indistinguishable from a
            // body that happened to be that size.
            *truncated = true;
        }
        if room > 0 {
            buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
    }

    /// Mark the capture as incomplete without adding bytes — for a body that was
    /// spooled, where only the in-memory prefix was kept.
    fn mark_truncated(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.1 = true;
        }
    }

    fn take(&self) -> Vec<u8> {
        self.0.lock().map(|s| s.0.clone()).unwrap_or_default()
    }

    fn truncated(&self) -> bool {
        self.0.lock().map(|s| s.1).unwrap_or(false)
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

/// The one HTTP client Grove uses to talk to upstreams (`Driver::Proxy`) and to
/// itself when replaying.
///
/// Built once, on purpose. A `Client` *is* the connection pool, so constructing
/// one per request threw the pool away every time: every proxied request paid a
/// fresh TCP handshake, and a Vite dev server saw a new connection per asset
/// instead of a handful of kept-alive ones. On a page with a hundred module
/// requests that is a hundred avoidable handshakes.
fn shared_client() -> &'static hyper_util::client::legacy::Client<
    hyper_util::client::legacy::connect::HttpConnector,
    BoxBody,
> {
    use hyper_util::client::legacy::{connect::HttpConnector, Client};
    use hyper_util::rt::TokioExecutor;

    static CLIENT: once_cell::sync::Lazy<Client<HttpConnector, BoxBody>> =
        once_cell::sync::Lazy::new(|| {
            let mut connector = HttpConnector::new();
            connector.set_nodelay(true);
            Client::builder(TokioExecutor::new())
                .pool_idle_timeout(std::time::Duration::from_secs(30))
                .pool_max_idle_per_host(32)
                .build(connector)
        });
    &CLIENT
}

/// Handle one incoming request end to end. Never panics — every error path
/// becomes an HTTP status so one bad site can't take down the daemon.
pub async fn handle(
    mut req: Request<Incoming>,
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

    let host = request_host(&req);
    // HTTP/2 carries the host in the `:authority` pseudo-header and has no
    // `Host` at all. Everything downstream — FastCGI's HTTP_HOST, the proxy
    // driver's X-Forwarded-Host, the timeline capture that replay rebuilds
    // from — reads the header map, so put it there once rather than teaching
    // each of them about h2. (Only when absent: a client that sent both is
    // left alone.)
    if !host.is_empty() && !req.headers().contains_key(hyper::header::HOST) {
        if let Ok(hv) = hyper::header::HeaderValue::from_str(&host) {
            req.headers_mut().insert(hyper::header::HOST, hv);
        }
    }

    // A tunnel keeps the public Host (so the app builds correct asset URLs) and
    // carries the local site name in X-Grove-Site purely for routing.
    let route_host = req
        .headers()
        .get("x-grove-site")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty() && trusts_forwarded_headers(&peer))
        .unwrap_or_else(|| host.clone());

    // Honour X-Forwarded-Proto so generated URLs use https — but only from the
    // tunnel, which reaches the site over loopback. From anywhere else it is
    // just a string the caller chose, and the proxy binds 0.0.0.0: without this
    // check anyone on the same network could convince an app that its plaintext
    // request arrived over TLS, which is enough to have it set secure cookies on
    // an insecure connection or build https asset URLs for a site with no TLS.
    let https = https
        || (trusts_forwarded_headers(&peer)
            && req
                .headers()
                .get("x-forwarded-proto")
                .and_then(|h| h.to_str().ok())
                .map(|v| v.eq_ignore_ascii_case("https"))
                .unwrap_or(false));

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

    // The 2 GiB ceiling was enforced only while *measuring* a chunked body; a
    // request that declared its length up front streamed through unchecked.
    if let Some(n) = exact {
        if n > MAX_REQUEST_BODY {
            return Ok(error_page(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!(
                    "The request body exceeds Grove's {} MiB limit",
                    MAX_REQUEST_BODY / (1024 * 1024)
                ),
                None,
            ));
        }
    }

    let buffered = |bytes: Bytes| {
        let tap = BodyTap::default();
        tap.push(&bytes);
        let len = bytes.len() as u64;
        (tap, len, full(bytes))
    };

    let (tap, body_len, forwarded) = match exact {
        // Declared and larger than the timeline could keep: stream it.
        Some(n) if n > reqlog::MAX_BODY as u64 => {
            let tap = BodyTap::default();
            (tap.clone(), n, tapped_body(body, tap))
        }
        // Declared and small: unchanged from before.
        Some(_) => {
            let bytes = body
                .collect()
                .await
                .map(|b| b.to_bytes())
                .unwrap_or_default();
            buffered(bytes)
        }
        // No declared length (chunked). CGI needs the number up front, so the
        // body has to be measured before it can be forwarded.
        None => match read_unsized_body(body).await {
            Ok(Unsized::Buffered(bytes)) => buffered(bytes),
            Ok(Unsized::Spooled { file, len, prefix }) => match spooled_body(file).await {
                Ok(body) => {
                    let tap = BodyTap::default();
                    tap.push(&prefix);
                    if len > prefix.len() as u64 {
                        tap.mark_truncated();
                    }
                    (tap, len, body)
                }
                Err(e) => {
                    tracing::error!(error = %e, "reading spooled request body failed");
                    return Ok(error_page(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Grove could not read the spooled request body",
                        Some("Check free disk space and the daemon log (`grove logs daemon`)."),
                    ));
                }
            },
            Ok(Unsized::TooLarge) => {
                return Ok(error_page(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    &format!(
                        "The request body exceeds Grove's {} MiB limit",
                        MAX_REQUEST_BODY / (1024 * 1024)
                    ),
                    None,
                ));
            }
            Err(e) => {
                tracing::warn!(error = %e, "reading request body failed");
                return Ok(error_page(
                    StatusCode::BAD_REQUEST,
                    "Grove could not read the request body",
                    None,
                ));
            }
        },
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
            body_truncated: tap.truncated(),
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
            body_truncated: tap.truncated(),
        });
        return Ok(error_page(
            StatusCode::NOT_FOUND,
            &format!("No site is registered for {route_host}"),
            Some("grove link          # in the project directory\ngrove park ~/Code   # every subdirectory becomes a site"),
        ));
    };

    // A site the user ran `grove secure` on is meant to be reached over TLS,
    // but a typed hostname still defaults to http:// in every browser. Serving
    // that request in plaintext handed PHP `HTTPS=""`, so Laravel built
    // http:// asset URLs — mixed content against the HTTPS Vite server — and
    // apps set their session cookie without `Secure`. Valet redirects; so does
    // Grove. `https` here already accounts for the tunnel's trusted
    // X-Forwarded-Proto, so a request that arrived over TLS somewhere upstream
    // is not bounced.
    if site.secure && !https {
        let resp = https_redirect(&method, &host, &path, state.https_port);
        state.log.record(Record {
            site: &site.name,
            host: &host,
            method: &method,
            path: &path,
            status: resp.status().as_u16(),
            duration_ms: start.elapsed().as_millis() as u64,
            https,
            headers: req_headers,
            body: tap.take(),
            body_truncated: tap.truncated(),
        });
        return Ok(resp);
    }

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
            body_truncated: tap.truncated(),
        });
        return Ok(error_page(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("The container behind {} is stopped", site.hostname),
            Some("Start it from Grove's Sites list, or with `docker start`."),
        ));
    }

    tracing::debug!(host, site = %site.name, driver = %site.driver, %peer, "dispatch");

    let result = if is_hidden_path(&sanitize_path(req.uri().path())) {
        // `.env`, `.git/config`, editor backups: never served, never executed.
        Ok(error_page(StatusCode::NOT_FOUND, "Not found", None))
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
                // Async stat: `Path::is_file` blocks the runtime worker, and this
                // one runs on *every* request to a PHP site.
                let is_file = !rel.as_os_str().is_empty()
                    && tokio::fs::metadata(&candidate)
                        .await
                        .is_ok_and(|m| m.is_file());
                if is_file {
                    if is_php_script(&rel) {
                        // Execute it, never serve it. Reading it out as text would
                        // disclose source (`/index.php` leaked the front controller)
                        // and would break any app that addresses scripts directly,
                        // such as WordPress's wp-login.php and wp-admin/*.php.
                        serve_php(req, &site, &fpm, https, Some(rel), body_len).await
                    } else {
                        serve_static(req, &site).await
                    }
                } else {
                    serve_php(req, &site, &fpm, https, None, body_len).await
                }
            }
            _ => serve_static(req, &site).await,
        }
    };

    let response = result.unwrap_or_else(|e| {
        tracing::error!(error = %e, site = %site.name, "request failed");
        upstream_error_page(&site, &e)
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
        body_truncated: tap.truncated(),
    });
    Ok(response)
}

/// Re-issue a captured request through Grove's own HTTP port so it flows through
/// the full proxy pipeline again (and is logged as a fresh entry). Routing is by
/// the original `Host` header. Returns `(status, duration_ms)`.
pub async fn replay(http_port: u16, cap: &CapturedRequest) -> anyhow::Result<(u16, u64)> {
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
    let request = builder.body(full(cap.body.clone()))?;

    let start = std::time::Instant::now();
    let resp = shared_client().request(request).await?;
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

/// Above this, a static file is streamed from disk instead of read into memory.
///
/// A 200 MB video or sourcemap in `public/` used to be held in RAM in full,
/// per concurrent request, before the first byte reached the browser.
const STATIC_STREAM_THRESHOLD: u64 = 256 * 1024;

/// A weak validator built from the facts a `stat` already gives us: size and
/// mtime. Cheap (no hashing, no second read) and exactly what nginx does.
fn etag_for(meta: &std::fs::Metadata) -> Option<String> {
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(format!("\"{:x}-{:x}\"", mtime.as_secs(), meta.len()))
}

/// Serve a static file from the document root, with a directory-index fallback.
async fn serve_static(
    req: Request<BoxBody>,
    site: &ResolvedSite,
) -> Result<Response<BoxBody>, anyhow::Error> {
    let rel = sanitize_path(req.uri().path());
    let mut target = site.document_root.join(&rel);

    // `tokio::fs::metadata`, not `Path::is_dir`/`exists`: those are blocking
    // syscalls issued straight from the async task, so a cold cache or a slow
    // volume (a network share, a Docker bind mount) stalls the whole runtime
    // worker and with it every other site's requests.
    let mut meta = tokio::fs::metadata(&target).await.ok();
    if meta.as_ref().is_some_and(|m| m.is_dir()) {
        target = target.join("index.html");
        meta = tokio::fs::metadata(&target).await.ok();
    }
    let meta = match meta {
        Some(m) => m,
        None => {
            // SPA fallback to the root index — but only for what a browser
            // navigation looks like: a path with no file extension, from a
            // client that accepts HTML. It used to apply to *every* missing
            // path, so a stale hashed asset (`/assets/app-abc123.js`) came
            // back as `200 text/html` and the browser said "expected a
            // JavaScript MIME type" instead of the 404 that names the problem.
            let fallback = site.document_root.join("index.html");
            let navigation = !has_extension(req.uri().path()) && accepts_html(&req);
            match tokio::fs::metadata(&fallback).await {
                Ok(m) if navigation => {
                    target = fallback;
                    m
                }
                _ => {
                    return Ok(error_page(
                        StatusCode::NOT_FOUND,
                        &format!("{} has no file at {}", site.hostname, req.uri().path()),
                        None,
                    ))
                }
            }
        }
    };

    let etag = etag_for(&meta);
    // A dev site reloads the same unchanged assets constantly. Without a
    // validator the browser cannot ask "has this changed?", so every reload
    // re-read and re-sent every byte; with one, unchanged files cost a 304.
    if let Some(etag) = &etag {
        let matches = req
            .headers()
            .get(hyper::header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                let gz = format!("{}-gz\"", etag.trim_end_matches('"'));
                v.split(',')
                    .map(str::trim)
                    .any(|c| c == etag.as_str() || c == gz)
            });
        if matches {
            return Ok(Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(hyper::header::ETAG, etag)
                .body(full(Bytes::new()))?);
        }
    }

    let mime = mime_for(&target);
    let total = meta.len();
    let base = || {
        Response::builder()
            .header(hyper::header::CONTENT_TYPE, mime)
            // Local development: never let a stale asset outlive an edit. The
            // validator above is what makes repeat loads cheap, not a max-age.
            .header(hyper::header::CACHE_CONTROL, "no-cache")
            // Safari refuses to play <video> from a server that will not do 206.
            .header(hyper::header::ACCEPT_RANGES, "bytes")
    };
    // The ETag is per representation: the gzip variant must carry a different
    // validator, and `Builder::header` appends rather than replaces, so it is
    // added last, once, by whichever branch answers.
    let with_etag = |b: hyper::http::response::Builder, suffix: &str| match &etag {
        Some(e) => b.header(
            hyper::header::ETAG,
            format!("{}{suffix}\"", e.trim_end_matches('"')),
        ),
        None => b,
    };

    // Range requests: video/audio scrubbing, resumed downloads. One range only,
    // which is all browsers send.
    if let Some(range) = req
        .headers()
        .get(hyper::header::RANGE)
        .and_then(|v| v.to_str().ok())
    {
        return match parse_byte_range(range, total) {
            Some((start, end)) => {
                let len = end - start + 1;
                let mut file = tokio::fs::File::open(&target).await?;
                {
                    use tokio::io::AsyncSeekExt;
                    file.seek(std::io::SeekFrom::Start(start)).await?;
                }
                Ok(with_etag(base(), "")
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(
                        hyper::header::CONTENT_RANGE,
                        format!("bytes {start}-{end}/{total}"),
                    )
                    .header(hyper::header::CONTENT_LENGTH, len)
                    .body(file_body_limited(file, len))?)
            }
            None => Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(hyper::header::CONTENT_RANGE, format!("bytes */{total}"))
                .body(full(Bytes::new()))?),
        };
    }

    if total > STATIC_STREAM_THRESHOLD {
        let file = tokio::fs::File::open(&target).await?;
        return Ok(with_etag(base(), "")
            .status(StatusCode::OK)
            .header(hyper::header::CONTENT_LENGTH, total)
            .body(file_body(file))?);
    }

    let bytes = tokio::fs::read(&target).await?;

    // Compress what compresses, for clients that ask. A 2 MB dev bundle went
    // out as 2 MB. Only the in-memory path — anything larger streams as-is —
    // and only when it actually gets smaller.
    if is_compressible(mime) && accepts_gzip(&req) {
        if let Some(gz) = gzip(&bytes) {
            return Ok(with_etag(base(), "-gz")
                .status(StatusCode::OK)
                .header(hyper::header::CONTENT_ENCODING, "gzip")
                .header(hyper::header::VARY, "Accept-Encoding")
                .header(hyper::header::CONTENT_LENGTH, gz.len())
                .body(full(gz))?);
        }
    }

    Ok(with_etag(base(), "")
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_LENGTH, total)
        .header(hyper::header::VARY, "Accept-Encoding")
        .body(full(bytes))?)
}

/// Whether the last path segment carries a file extension — the cheap signal
/// that a request is for an asset rather than a route.
fn has_extension(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .map(|seg| seg.contains('.') && !seg.ends_with('.'))
        .unwrap_or(false)
}

/// A browser navigation says `text/html` (or `*/*` from older clients).
/// `fetch()` of a module says `*/*`, but it also has an extension — see
/// [`has_extension`]; both conditions gate the SPA fallback.
fn accepts_html<B>(req: &Request<B>) -> bool {
    match req
        .headers()
        .get(hyper::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
    {
        None => true,
        Some(a) => a.contains("text/html") || a.contains("*/*"),
    }
}

fn accepts_gzip<B>(req: &Request<B>) -> bool {
    req.headers()
        .get(hyper::header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',')
                .any(|e| e.trim().split(';').next() == Some("gzip"))
        })
        .unwrap_or(false)
}

fn is_compressible(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.starts_with("application/json")
        || mime.starts_with("application/javascript")
        || mime.starts_with("application/xml")
        || mime.starts_with("application/manifest+json")
        || mime == "image/svg+xml"
}

/// gzip `bytes`, or `None` when compression would not help.
fn gzip(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(
        Vec::with_capacity(bytes.len() / 2),
        flate2::Compression::fast(),
    );
    enc.write_all(bytes).ok()?;
    let out = enc.finish().ok()?;
    (out.len() < bytes.len()).then_some(out)
}

/// Parse a single `bytes=` range against a resource of `total` bytes into an
/// inclusive `(start, end)`. `None` means unsatisfiable (or a multi-range,
/// which nothing in local development sends).
fn parse_byte_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || total == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            // Suffix: the last n bytes.
            let n: u64 = n.parse().ok()?;
            if n == 0 {
                return None;
            }
            (total.saturating_sub(n), total - 1)
        }
        (s, "") => (s.parse().ok()?, total - 1),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(total - 1)),
    };
    (start <= end && start < total).then_some((start, end))
}

/// Stream at most `limit` bytes of `file`, for a range response.
fn file_body_limited(file: tokio::fs::File, limit: u64) -> BoxBody {
    use tokio::io::AsyncReadExt;
    file_body_from_reader(file.take(limit))
}

/// Stream a file as a response body in fixed-size chunks.
fn file_body(file: tokio::fs::File) -> BoxBody {
    file_body_from_reader(file)
}

fn file_body_from_reader<R>(reader: R) -> BoxBody
where
    R: tokio::io::AsyncRead + Unpin + Send + Sync + 'static,
{
    use futures::StreamExt;
    use tokio::io::AsyncReadExt;

    let stream =
        futures::stream::unfold(Some((reader, vec![0u8; 64 * 1024])), |state| async move {
            let (mut file, mut buf) = state?;
            match file.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    let chunk = Bytes::copy_from_slice(&buf[..n]);
                    Some((Ok(chunk), Some((file, buf))))
                }
                Err(e) => Some((Err(e), None)),
            }
        })
        .map(|chunk| chunk.map(hyper::body::Frame::data));

    BodyExt::boxed(http_body_util::StreamBody::new(stream))
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
    fpm: &Arc<dyn FpmLocator>,
    https: bool,
    direct: Option<PathBuf>,
    body_len: u64,
) -> Result<Response<BoxBody>, anyhow::Error> {
    // `locate` is synchronous and, when a pool has to be spawned, genuinely
    // blocking: it forks php-fpm and then polls for its socket to appear for up
    // to a second. Called directly, that parks a runtime worker for the whole
    // wait — stalling every other site's requests that happen to be scheduled on
    // it — so it belongs on the blocking pool.
    let addr = {
        let fpm = fpm.clone();
        let version = site.php.clone();
        tokio::task::spawn_blocking(move || fpm.locate(&version))
            .await
            .unwrap_or(None)
    };
    let Some(addr) = addr else {
        return Ok(error_page(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("No PHP {} is available for {}", site.php, site.hostname),
            Some(&format!(
                "grove php install {v}        # install it\ngrove isolate {s} <version>   # or pin this site to one you have",
                v = site.php,
                s = site.name
            )),
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
    let Some(upstream) = &site.proxy_to else {
        return Ok(error_page(
            StatusCode::BAD_GATEWAY,
            &format!("{} is a proxy site with no upstream", site.hostname),
            Some(&format!(
                "grove proxy {} http://127.0.0.1:<port>",
                site.name
            )),
        ));
    };

    // A protocol switch cannot go through the pooled client: it has no way to
    // hand the raw connection back. WebSockets get their own connection.
    if is_upgrade_request(&req) {
        return proxy_upgrade(req, site, upstream).await;
    }

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

    let resp = shared_client().request(forwarded).await?;
    let (parts, body) = resp.into_parts();
    // Pass the upstream body through without collecting it, so a Vite HMR stream
    // or a Node SSE endpoint reaches the client as it arrives.
    let body = body.map_err(std::io::Error::other).boxed();
    Ok(Response::from_parts(parts, body))
}

/// Whether a request asks to switch protocols — a WebSocket handshake, in
/// practice. Both headers are required by RFC 9110 §7.8; checking only
/// `Upgrade` would catch clients that advertise without asking.
fn is_upgrade_request<B>(req: &Request<B>) -> bool {
    let wants_upgrade = req.headers().contains_key(hyper::header::UPGRADE);
    let connection_says_so = req
        .headers()
        .get_all(hyper::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        });
    wants_upgrade && connection_says_so
}

/// Pass a WebSocket (or any HTTP Upgrade) through to a proxy site's upstream.
///
/// The listeners have accepted upgrades all along (`with_upgrades`), but
/// nothing consumed them: `serve_proxy` used the pooled `Client`, which cannot
/// upgrade, so the browser got a `101` with no byte pump behind it — the
/// socket opened and died, and Vite HMR on a proxy site, Next/Nuxt dev
/// servers, Reverb over `wss://myapp.test`, all reconnect-looped.
///
/// Dedicated connection instead: send the handshake over a fresh HTTP/1.1
/// connection to the upstream, and if it answers `101`, wait for both sides'
/// upgrades to complete and copy bytes between them until either closes.
/// Anything other than `101` is passed back as an ordinary response.
async fn proxy_upgrade(
    mut req: Request<BoxBody>,
    site: &ResolvedSite,
    upstream: &str,
) -> Result<Response<BoxBody>, anyhow::Error> {
    use anyhow::Context as _;
    use hyper_util::rt::TokioIo;

    // Must be taken before the request is consumed; it is the handle to the
    // client's connection once the 101 goes out.
    let client_upgrade = hyper::upgrade::on(&mut req);

    let target: hyper::Uri = upstream.parse().context("upstream URL")?;
    if target.scheme_str() == Some("https") {
        anyhow::bail!("WebSocket upgrade to an https upstream is not supported yet ({upstream})");
    }
    let host = target
        .host()
        .context("upstream URL has no host")?
        .to_string();
    let port = target.port_u16().unwrap_or(80);
    let stream = tokio::net::TcpStream::connect((host.as_str(), port))
        .await
        .with_context(|| format!("tcp connect error to {host}:{port}"))?;
    let _ = stream.set_nodelay(true);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .context("upstream HTTP/1.1 handshake")?;
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            tracing::debug!(error = %e, "upstream websocket connection ended");
        }
    });

    // Origin-form request line and the upstream's own Host, as for any proxied
    // request; the forwarding headers so the app knows its public name.
    let (mut parts, body) = req.into_parts();
    let path_q = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let public_host = parts
        .headers
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    parts.uri = path_q.parse().context("request path")?;
    let authority = if port == 80 {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    parts.headers.insert(
        hyper::header::HOST,
        hyper::header::HeaderValue::from_str(&authority)?,
    );
    if let Some(h) = public_host.and_then(|h| hyper::header::HeaderValue::from_str(&h).ok()) {
        parts.headers.insert("x-forwarded-host", h);
    }
    parts.headers.insert(
        "x-forwarded-proto",
        hyper::header::HeaderValue::from_static("https"),
    );
    let upstream_req = Request::from_parts(parts, body);

    let mut resp = sender
        .send_request(upstream_req)
        .await
        .with_context(|| format!("websocket handshake with {upstream}"))?;

    if resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        // The upstream declined (or answered the request normally). Hand its
        // answer back unchanged; the client never switched.
        let (parts, body) = resp.into_parts();
        return Ok(Response::from_parts(
            parts,
            body.map_err(std::io::Error::other).boxed(),
        ));
    }

    let upstream_upgrade = hyper::upgrade::on(&mut resp);
    let site_name = site.name.clone();
    tokio::spawn(async move {
        match tokio::try_join!(client_upgrade, upstream_upgrade) {
            Ok((client, upstream)) => {
                let mut client = TokioIo::new(client);
                let mut upstream = TokioIo::new(upstream);
                match tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
                    Ok((up, down)) => tracing::debug!(
                        site = %site_name,
                        bytes_up = up,
                        bytes_down = down,
                        "websocket closed"
                    ),
                    Err(e) => tracing::debug!(site = %site_name, error = %e, "websocket ended"),
                }
            }
            Err(e) => {
                tracing::debug!(site = %site_name, error = %e, "websocket upgrade did not complete")
            }
        }
    });

    // The 101 goes back with the upstream's headers (Sec-WebSocket-Accept and
    // friends); hyper performs the client-side switch once it is written.
    let (parts, _) = resp.into_parts();
    Ok(Response::from_parts(parts, full(Bytes::new())))
}

/// Build the CGI/1.1 environment FastCGI expects.
/// Whether Grove's own forwarded hints from this peer may be believed.
///
/// `x-forwarded-proto` and `x-grove-site` are injected by Grove's tunnel, which
/// proxies into the site over loopback (`grove share` runs on the same machine as
/// the daemon). Every other source is an ordinary client, and the HTTP/HTTPS
/// listeners bind `0.0.0.0` — so on the LAN these are attacker-controlled.
///
/// Loopback is not an authorization boundary in general: any local process can
/// reach it. It is the right line *here* because these headers only change how a
/// request is described to an app the local user already controls, and a local
/// process could reach that app directly anyway. What it removes is the remote
/// attacker.
fn trusts_forwarded_headers(peer: &SocketAddr) -> bool {
    peer.ip().is_loopback()
}

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

    // Forward request headers as HTTP_* CGI variables.
    //
    // The `HTTP_` prefix normally keeps client input from colliding with real
    // environment variables — except for one name, which is why `Proxy` is
    // dropped here. A client-supplied `Proxy:` header becomes `HTTP_PROXY`,
    // which is precisely the variable HTTP clients consult for an outbound
    // proxy, so any PHP that reads it (directly, or through a library that has
    // not been hardened) would send its outbound requests wherever the *caller*
    // chose. That is httpoxy, CVE-2016-5385; nginx and Apache strip it too.
    //
    // A legitimate request has no reason to carry it: `Proxy` is not a real
    // request header in any specification.
    for (name, value) in parts.headers.iter() {
        if name.as_str().eq_ignore_ascii_case("proxy") {
            continue;
        }
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
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml; charset=utf-8",
        Some("map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("csv") => "text/csv; charset=utf-8",
        Some("pdf") => "application/pdf",
        Some("avif") => "image/avif",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// The host a request is for, whichever way the protocol carried it.
///
/// HTTP/1.1 sends a `Host` header. HTTP/2 sends `:authority` instead — hyper
/// surfaces that as the request URI's authority and leaves `Host` unset — so a
/// handler that only reads the header sees `""` for every h2 request and 404s
/// each site the moment a browser negotiates h2. Enabling HTTP/2 without this
/// was exactly that: green in unit tests whose service ignored the host, a
/// 404 on the first real curl.
fn request_host<B>(req: &Request<B>) -> String {
    if let Some(h) = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| !h.is_empty())
    {
        return h.to_string();
    }
    req.uri()
        .authority()
        .map(|a| a.as_str().to_string())
        .unwrap_or_default()
}

/// Send a plaintext request for a secured site to its HTTPS origin.
///
/// The `Host` header may carry the HTTP listener's port; the redirect must
/// carry the HTTPS one instead, and omit it when it is the default. `301` for
/// GET/HEAD so browsers cache the upgrade; `308` otherwise so a POST that hit
/// the wrong scheme keeps its method and body rather than turning into a GET.
fn https_redirect(method: &str, host: &str, path: &str, https_port: u16) -> Response<BoxBody> {
    let hostname = match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    };
    let location = if https_port == 443 {
        format!("https://{hostname}{path}")
    } else {
        format!("https://{hostname}:{https_port}{path}")
    };
    let status = if method == "GET" || method == "HEAD" {
        StatusCode::MOVED_PERMANENTLY
    } else {
        StatusCode::PERMANENT_REDIRECT
    };
    Response::builder()
        .status(status)
        .header(hyper::header::LOCATION, &location)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full(format!("Redirecting to {location}\n")))
        .expect("static redirect response builds")
}

/// Minimal HTML escaping for the few user-controlled strings that reach an
/// error page (a hostname, an upstream URL, a PHP version).
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// An error page that says what happened and what to do about it.
///
/// Every Grove-generated error used to be a bare `text/plain` line — "Grove:
/// client error (Connect): tcp connect error … (os error 61)" in the browser
/// when a Vite server was not running. Herd and Valet ship pages that name the
/// fix; so does Grove now. `detail` is the specific situation, `hint` the
/// command or action that resolves it. Both are escaped.
fn error_page(status: StatusCode, detail: &str, hint: Option<&str>) -> Response<BoxBody> {
    let reason = status.canonical_reason().unwrap_or("Error");
    let hint_html = hint
        .map(|h| format!("<p class=\"hint\">{}</p>", html_escape(h)))
        .unwrap_or_default();
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{code} {reason} — Grove</title>\
<style>\
:root{{color-scheme:light dark}}\
body{{margin:0;min-height:100vh;display:grid;place-items:center;font:16px/1.5 ui-sans-serif,system-ui,sans-serif;\
background:#f6f7f5;color:#1d221c}}\
@media(prefers-color-scheme:dark){{body{{background:#161a15;color:#e6e9e3}}}}\
main{{max-width:36rem;padding:2rem 2.5rem}}\
.code{{font-size:.8rem;letter-spacing:.08em;text-transform:uppercase;opacity:.6}}\
h1{{margin:.25rem 0 1rem;font-size:1.5rem;font-weight:600}}\
p{{margin:.5rem 0}}\
.hint{{margin-top:1.25rem;padding:.75rem 1rem;border-left:3px solid #4c8a4a;background:rgba(76,138,74,.08);\
font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.9rem;white-space:pre-wrap}}\
footer{{margin-top:2rem;font-size:.8rem;opacity:.5}}\
</style></head><body><main>\
<div class=\"code\">{code} {reason}</div>\
<h1>{detail}</h1>\
{hint_html}\
<footer>Grove</footer>\
</main></body></html>",
        code = status.as_u16(),
        reason = html_escape(reason),
        detail = html_escape(detail),
    );
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(hyper::header::CACHE_CONTROL, "no-store")
        .body(full(body))
        .expect("static error page builds")
}

/// The 502 a proxy or FastCGI failure turns into. A refused connection on a
/// proxy site is by far the most common cause — the dev server is simply not
/// running — and that case names the upstream and how to start it.
fn upstream_error_page(site: &ResolvedSite, err: &anyhow::Error) -> Response<BoxBody> {
    let text = format!("{err:#}");
    let refused = text.contains("Connection refused")
        || text.contains("os error 61")
        || text.contains("os error 111")
        || text.contains("tcp connect error");
    match (&site.driver, &site.proxy_to) {
        (Driver::Proxy, Some(upstream)) if refused => error_page(
            StatusCode::BAD_GATEWAY,
            &format!("Nothing is listening at {upstream}"),
            Some(&format!(
                "The dev server behind {} isn't running.\n\ngrove dev start {}   # if Grove manages it\nnpm run dev            # or start it yourself",
                site.hostname, site.name
            )),
        ),
        (Driver::Proxy, Some(upstream)) => error_page(
            StatusCode::BAD_GATEWAY,
            &format!("The upstream at {upstream} failed"),
            Some(&text),
        ),
        _ => error_page(
            StatusCode::BAD_GATEWAY,
            &format!("{} could not be served", site.hostname),
            Some(&text),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the `Parts` of a request carrying `headers`, for the CGI-parameter
    /// tests below.
    fn parts_with(headers: &[(&str, &str)]) -> hyper::http::request::Parts {
        let mut b = hyper::Request::builder().uri("http://myapp.test/index.php");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(()).unwrap().into_parts().0
    }

    fn fcgi_params(headers: &[(&str, &str)]) -> HashMap<String, String> {
        let parts = parts_with(headers);
        let site = ResolvedSite::from_parts(
            "myapp".to_string(),
            "test",
            PathBuf::from("/srv"),
            grove_core::driver::DriverPlan {
                driver: grove_core::Driver::Laravel,
                document_root: PathBuf::from("/srv/public"),
                front_controller: Some(PathBuf::from("index.php")),
            },
            "8.5".to_string(),
            None,
            false,
            grove_core::SiteKind::Linked,
            None,
        );
        build_fcgi_params(
            &parts,
            &site,
            Path::new("/srv/public/index.php"),
            Path::new("/index.php"),
            false,
            0,
            false,
        )
    }

    /// httpoxy (CVE-2016-5385): a client-supplied `Proxy:` header must not
    /// become `HTTP_PROXY`, which is the variable outbound HTTP clients read.
    #[test]
    fn the_proxy_header_never_reaches_php() {
        for name in ["Proxy", "proxy", "PROXY", "pRoXy"] {
            let p = fcgi_params(&[(name, "http://attacker.example:3128")]);
            assert!(
                !p.contains_key("HTTP_PROXY"),
                "{name}: HTTP_PROXY leaked through as {:?}",
                p.get("HTTP_PROXY")
            );
        }
    }

    /// …while every other header still does, so the strip is surgical.
    #[test]
    fn ordinary_headers_still_reach_php() {
        let p = fcgi_params(&[
            ("Proxy", "http://attacker.example:3128"),
            ("X-Proxy-Authorization", "keep-me"),
            ("Accept", "text/html"),
            ("X-Custom-Thing", "value"),
        ]);
        assert!(!p.contains_key("HTTP_PROXY"));
        // A header whose name merely *contains* "proxy" is untouched: the
        // collision is with the exact name, not the substring.
        assert_eq!(
            p.get("HTTP_X_PROXY_AUTHORIZATION").map(String::as_str),
            Some("keep-me")
        );
        assert_eq!(p.get("HTTP_ACCEPT").map(String::as_str), Some("text/html"));
        assert_eq!(
            p.get("HTTP_X_CUSTOM_THING").map(String::as_str),
            Some("value")
        );
    }

    #[test]
    fn forwarded_hints_are_believed_only_over_loopback() {
        for addr in ["127.0.0.1:9000", "[::1]:9000"] {
            let peer: SocketAddr = addr.parse().unwrap();
            assert!(
                trusts_forwarded_headers(&peer),
                "{addr} is the tunnel's own path and must be trusted"
            );
        }
        // The listeners bind 0.0.0.0, so these are reachable from the LAN.
        for addr in [
            "192.168.1.50:9000",
            "10.0.0.7:9000",
            "[2001:db8::1]:9000",
            "8.8.8.8:9000",
        ] {
            let peer: SocketAddr = addr.parse().unwrap();
            assert!(
                !trusts_forwarded_headers(&peer),
                "{addr} must not be able to claim its request was HTTPS"
            );
        }
    }

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

    fn static_site(root: PathBuf) -> ResolvedSite {
        ResolvedSite {
            name: "t".into(),
            hostname: "t.test".into(),
            path: root.clone(),
            document_root: root,
            driver: Driver::Static,
            php: "8.3".into(),
            node: None,
            secure: false,
            kind: grove_core::site::SiteKind::Linked,
            proxy_to: None,
            front_controller: None,
            docker: false,
            docker_id: None,
            docker_running: false,
        }
    }

    fn get(path: &str, etag: Option<&str>) -> Request<BoxBody> {
        let mut b = Request::builder().uri(path);
        if let Some(e) = etag {
            b = b.header(hyper::header::IF_NONE_MATCH, e);
        }
        b.body(full(Bytes::new())).unwrap()
    }

    #[tokio::test]
    async fn unchanged_assets_answer_304_on_revalidation() {
        // Regression: without a validator, every reload of an unchanged asset
        // re-read the file and re-sent every byte.
        let dir = std::env::temp_dir().join(format!("grove-static-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("app.css"), b"body{}").unwrap();
        let site = static_site(dir.clone());

        let first = serve_static(get("/app.css", None), &site).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first
            .headers()
            .get(hyper::header::ETAG)
            .expect("etag is set")
            .to_str()
            .unwrap()
            .to_string();

        let second = serve_static(get("/app.css", Some(&etag)), &site)
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);

        // A changed file must not be served from the browser's copy.
        std::fs::write(dir.join("app.css"), b"body{color:red}").unwrap();
        let third = serve_static(get("/app.css", Some(&etag)), &site)
            .await
            .unwrap();
        assert_eq!(third.status(), StatusCode::OK);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn missing_files_are_still_404() {
        let dir = std::env::temp_dir().join(format!("grove-static-404-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let site = static_site(dir.clone());
        let resp = serve_static(get("/nope.css", None), &site).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(
            sanitize_path("/../../etc/passwd"),
            PathBuf::from("etc/passwd")
        );
        assert_eq!(sanitize_path("/css/app.css"), PathBuf::from("css/app.css"));
    }

    fn location_of(resp: &Response<BoxBody>) -> String {
        resp.headers()[hyper::header::LOCATION]
            .to_str()
            .unwrap()
            .to_string()
    }

    /// The everyday case: `myapp.test` typed into a browser, secured site,
    /// standard ports. The redirect must not carry `:443`, and must keep the
    /// path and query so a deep link survives the bounce.
    #[test]
    fn a_secured_site_over_http_redirects_to_its_https_origin() {
        let resp = https_redirect("GET", "myapp.test", "/admin?tab=2", 443);
        assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(location_of(&resp), "https://myapp.test/admin?tab=2");
    }

    /// The sudo-less smoke-test setup runs on high ports. A redirect to
    /// `https://myapp.test/` would then point at nothing, and the Host header
    /// carries the *HTTP* port, which must not leak into the Location.
    #[test]
    fn a_redirect_uses_the_https_port_not_the_one_in_host() {
        let resp = https_redirect("GET", "myapp.test:18080", "/", 18443);
        assert_eq!(location_of(&resp), "https://myapp.test:18443/");
    }

    /// A POST that hit the wrong scheme keeps its method and body.
    #[test]
    fn non_get_requests_redirect_with_308() {
        let resp = https_redirect("POST", "myapp.test", "/login", 443);
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(location_of(&resp), "https://myapp.test/login");
    }

    /// An IPv6 literal without a port has colons of its own and must not be
    /// mistaken for `host:port`.
    #[test]
    fn an_ipv6_host_is_not_split_on_its_colons() {
        let resp = https_redirect("GET", "[::1]", "/", 443);
        assert_eq!(location_of(&resp), "https://[::1]/");
    }

    /// HTTP/1.1: the Host header is the host.
    #[test]
    fn host_comes_from_the_header_on_http1() {
        let req = Request::builder()
            .uri("/")
            .header(hyper::header::HOST, "myapp.test")
            .body(full(Bytes::new()))
            .unwrap();
        assert_eq!(request_host(&req), "myapp.test");
    }

    /// HTTP/2: no Host header; the host is the URI authority (`:authority`).
    /// This is the case that 404'd every site the moment h2 was enabled.
    #[test]
    fn host_comes_from_the_authority_on_http2() {
        let req = Request::builder()
            .uri("https://myapp.test:18443/admin")
            .body(full(Bytes::new()))
            .unwrap();
        assert!(req.headers().get(hyper::header::HOST).is_none());
        assert_eq!(request_host(&req), "myapp.test:18443");
    }

    /// A client that sent both is left alone: the header wins, as on HTTP/1.1.
    #[test]
    fn an_explicit_host_header_wins_over_the_authority() {
        let req = Request::builder()
            .uri("https://other.test/")
            .header(hyper::header::HOST, "myapp.test")
            .body(full(Bytes::new()))
            .unwrap();
        assert_eq!(request_host(&req), "myapp.test");
    }

    fn static_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("grove-static-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<h1>app</h1>").unwrap();
        std::fs::write(dir.join("assets/app.js"), "console.log(1)".repeat(200)).unwrap();
        std::fs::write(dir.join("clip.mp4"), (0u8..=255).collect::<Vec<u8>>()).unwrap();
        dir
    }

    fn get_with(path: &str, headers: &[(&str, &str)]) -> Request<BoxBody> {
        let mut b = Request::builder().uri(path);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(full(Bytes::new())).unwrap()
    }

    /// A route with no extension, from a browser → the SPA index. A missing
    /// hashed asset → 404, not the index as text/html (which made the browser
    /// say "expected a JavaScript MIME type" instead of naming the file).
    #[tokio::test]
    async fn spa_fallback_serves_routes_but_not_missing_assets() {
        let dir = static_dir("spa");
        let site = static_site(dir.clone());

        let route = serve_static(
            get_with("/dashboard/users", &[("accept", "text/html,*/*")]),
            &site,
        )
        .await
        .unwrap();
        assert_eq!(route.status(), StatusCode::OK);
        assert!(route.headers()[hyper::header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));

        let asset = serve_static(
            get_with("/assets/app-abc123.js", &[("accept", "*/*")]),
            &site,
        )
        .await
        .unwrap();
        assert_eq!(
            asset.status(),
            StatusCode::NOT_FOUND,
            "a missing asset is a 404"
        );

        // An API-style client asking for JSON on an unknown route gets a 404 too.
        let api = serve_static(
            get_with("/api/things", &[("accept", "application/json")]),
            &site,
        )
        .await
        .unwrap();
        assert_eq!(api.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn compressible_assets_are_gzipped_only_when_asked() {
        let dir = static_dir("gzip");
        let site = static_site(dir.clone());

        let plain = serve_static(get_with("/assets/app.js", &[]), &site)
            .await
            .unwrap();
        assert!(plain
            .headers()
            .get(hyper::header::CONTENT_ENCODING)
            .is_none());
        let plain_len: usize = plain.headers()[hyper::header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();

        let gz = serve_static(
            get_with("/assets/app.js", &[("accept-encoding", "gzip, br")]),
            &site,
        )
        .await
        .unwrap();
        assert_eq!(gz.headers()[hyper::header::CONTENT_ENCODING], "gzip");
        assert_eq!(gz.headers()[hyper::header::VARY], "Accept-Encoding");
        let gz_len: usize = gz.headers()[hyper::header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(gz_len < plain_len, "{gz_len} < {plain_len}");
        assert_ne!(
            gz.headers()[hyper::header::ETAG],
            plain.headers()[hyper::header::ETAG],
            "a different representation needs a different validator"
        );

        // Already-compressed media is left alone even when gzip is accepted.
        let media = serve_static(get_with("/clip.mp4", &[("accept-encoding", "gzip")]), &site)
            .await
            .unwrap();
        assert!(media
            .headers()
            .get(hyper::header::CONTENT_ENCODING)
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn byte_ranges_are_honoured() {
        let dir = static_dir("range");
        let site = static_site(dir.clone());

        let whole = serve_static(get_with("/clip.mp4", &[]), &site)
            .await
            .unwrap();
        assert_eq!(whole.headers()[hyper::header::ACCEPT_RANGES], "bytes");

        let part = serve_static(get_with("/clip.mp4", &[("range", "bytes=10-19")]), &site)
            .await
            .unwrap();
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            part.headers()[hyper::header::CONTENT_RANGE],
            "bytes 10-19/256"
        );
        let body = part.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], &(10u8..=19).collect::<Vec<u8>>()[..]);

        let tail = serve_static(get_with("/clip.mp4", &[("range", "bytes=-4")]), &site)
            .await
            .unwrap();
        assert_eq!(
            tail.headers()[hyper::header::CONTENT_RANGE],
            "bytes 252-255/256"
        );

        let open_ended = serve_static(get_with("/clip.mp4", &[("range", "bytes=250-")]), &site)
            .await
            .unwrap();
        assert_eq!(
            open_ended.headers()[hyper::header::CONTENT_RANGE],
            "bytes 250-255/256"
        );

        let bad = serve_static(get_with("/clip.mp4", &[("range", "bytes=300-400")]), &site)
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(bad.headers()[hyper::header::CONTENT_RANGE], "bytes */256");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn range_parsing_edge_cases() {
        assert_eq!(parse_byte_range("bytes=0-0", 10), Some((0, 0)));
        assert_eq!(
            parse_byte_range("bytes=5-100", 10),
            Some((5, 9)),
            "end is clamped"
        );
        assert_eq!(
            parse_byte_range("bytes=-20", 10),
            Some((0, 9)),
            "suffix larger than file"
        );
        assert_eq!(
            parse_byte_range("bytes=10-", 10),
            None,
            "start past the end"
        );
        assert_eq!(
            parse_byte_range("bytes=0-1,5-6", 10),
            None,
            "multi-range unsupported"
        );
        assert_eq!(parse_byte_range("items=0-1", 10), None);
        assert_eq!(parse_byte_range("bytes=0-", 0), None, "empty file");
    }

    #[test]
    fn error_pages_escape_what_the_request_controlled() {
        let resp = error_page(
            StatusCode::NOT_FOUND,
            "No site for <script>evil</script>",
            Some("a & b"),
        );
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(resp.headers()[hyper::header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        let body = futures::executor::block_on(resp.into_body().collect())
            .unwrap()
            .to_bytes();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<script>evil"), "{html}");
        assert!(html.contains("a &amp; b"));
    }

    /// A WebSocket handshake through a proxy site, end to end: a real hyper
    /// server connection built like the listeners, `proxy_upgrade` as the
    /// service, and a bare TCP upstream that answers 101 and then echoes. The
    /// bytes written after the 101 must come back — that is the byte pump that
    /// was missing, and the reason HMR on proxy sites reconnect-looped.
    #[tokio::test]
    async fn a_websocket_upgrade_is_pumped_through_to_the_upstream() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Upstream: read the handshake, answer 101, echo everything after.
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let seen_by_upstream = seen.clone();
        tokio::spawn(async move {
            let (mut sock, _) = upstream.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                sock.read_exact(&mut byte).await.unwrap();
                buf.push(byte[0]);
            }
            *seen_by_upstream.lock().unwrap() = String::from_utf8_lossy(&buf).into_owned();
            sock.write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                  Connection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
            )
            .await
            .unwrap();
            let mut chunk = [0u8; 256];
            loop {
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if sock.write_all(&chunk[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Grove's side: the listeners' connection builder, serving proxy_upgrade.
        let site = ResolvedSite {
            driver: Driver::Proxy,
            proxy_to: Some(format!("http://{upstream_addr}")),
            ..static_site(PathBuf::from("/nonexistent"))
        };
        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        tokio::spawn(async move {
            let site = site.clone();
            let service = hyper::service::service_fn(move |req: Request<Incoming>| {
                let site = site.clone();
                async move {
                    let req = req.map(|b| b.map_err(std::io::Error::other).boxed());
                    let upstream = site.proxy_to.clone().unwrap();
                    Ok::<_, std::convert::Infallible>(
                        proxy_upgrade(req, &site, &upstream)
                            .await
                            .unwrap_or_else(|e| upstream_error_page(&site, &e)),
                    )
                }
            });
            let _ = crate::server::connection_builder()
                .serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(server_io), service)
                .await;
        });

        // The browser: a raw handshake, then bytes.
        let mut client = client_io;
        client
            .write_all(
                b"GET /app?token=abc HTTP/1.1\r\nHost: myapp.test\r\nUpgrade: websocket\r\n\
                  Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                  Sec-WebSocket-Version: 13\r\n\r\n",
            )
            .await
            .unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            client.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        assert!(
            head.to_ascii_lowercase()
                .contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
            "the upstream's handshake headers reach the client: {head}"
        );

        // After the switch, bytes flow both ways through Grove.
        client.write_all(b"ping from the browser").await.unwrap();
        let mut echoed = [0u8; 21];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut echoed),
        )
        .await
        .expect("the echo arrives")
        .unwrap();
        assert_eq!(&echoed[..], b"ping from the browser");

        // The upstream saw a proper proxied handshake: origin-form path with the
        // query, its own address as Host, the public host forwarded.
        let seen = seen.lock().unwrap().clone();
        assert!(
            seen.starts_with("GET /app?token=abc HTTP/1.1\r\n"),
            "{seen}"
        );
        let lower = seen.to_ascii_lowercase();
        assert!(lower.contains(&format!("host: {upstream_addr}")), "{seen}");
        assert!(lower.contains("x-forwarded-host: myapp.test"), "{seen}");
        assert!(lower.contains("upgrade: websocket"), "{seen}");
    }

    #[test]
    fn upgrade_detection_needs_both_headers() {
        let both = get_with(
            "/",
            &[
                ("upgrade", "websocket"),
                ("connection", "keep-alive, Upgrade"),
            ],
        );
        assert!(is_upgrade_request(&both));
        let only_upgrade = get_with("/", &[("upgrade", "websocket")]);
        assert!(!is_upgrade_request(&only_upgrade));
        let plain = get_with("/", &[("connection", "keep-alive")]);
        assert!(!is_upgrade_request(&plain));
    }
}
