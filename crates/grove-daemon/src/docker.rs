//! Auto-discover Docker / OrbStack containers and expose them as `<name>.test`.
//!
//! Grove already terminates TLS with a trusted CA and reverse-proxies, so a
//! container labelled for OrbStack (`dev.orbstack.domains`) or Grove
//! (`grove.host`) becomes a first-class `*.test` site with local HTTPS, right
//! next to native sites — and `grove share` can even tunnel it publicly.
//!
//! We talk to the Docker Engine API over its unix socket with a tiny HTTP/1.0
//! request (no extra deps): the daemon replies with the full JSON body and
//! closes, so we just read to EOF and parse.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// A container Grove will serve as `<name>.<tld>` by proxying to `upstream`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerSite {
    pub name: String,
    pub upstream: String,
}

/// Locate the Docker socket (honours `DOCKER_HOST`, standard paths, OrbStack).
fn socket_path() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("DOCKER_HOST") {
        if let Some(p) = h.strip_prefix("unix://") {
            let p = PathBuf::from(p);
            if p.exists() {
                return Some(p);
            }
        }
    }
    for p in ["/var/run/docker.sock", "/run/docker.sock"] {
        if Path::new(p).exists() {
            return Some(PathBuf::from(p));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".orbstack/run/docker.sock");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Discover container-backed sites. Returns an empty vec when Docker isn't
/// present, so this is safe to call unconditionally.
pub async fn discover() -> Vec<DockerSite> {
    // Never let a slow/absent Docker hang the daemon's refresh loop.
    tokio::time::timeout(std::time::Duration::from_secs(3), discover_inner())
        .await
        .unwrap_or_default()
}

async fn discover_inner() -> Vec<DockerSite> {
    let Some(sock) = socket_path() else {
        return Vec::new();
    };
    let Ok(mut stream) = UnixStream::connect(&sock).await else {
        return Vec::new();
    };
    // `Connection: close` makes the Engine close the socket after the response,
    // so `read_to_end` returns instead of blocking on a kept-alive connection.
    let req = b"GET /containers/json HTTP/1.0\r\nHost: localhost\r\nAccept: application/json\r\nConnection: close\r\n\r\n";
    if stream.write_all(req).await.is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if stream.read_to_end(&mut buf).await.is_err() {
        return Vec::new();
    }
    let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Vec::new();
    };
    let header = &buf[..pos];
    let body_raw = &buf[pos + 4..];
    // Docker may still reply chunked; decode if so.
    let decoded;
    let body: &[u8] = if header_has_chunked(header) {
        decoded = dechunk(body_raw);
        &decoded
    } else {
        body_raw
    };
    let Ok(list) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for c in list.as_array().into_iter().flatten() {
        if c.get("State").and_then(|s| s.as_str()) != Some("running") {
            continue;
        }
        let Some(labels) = c.get("Labels").and_then(|l| l.as_object()) else {
            continue;
        };
        let get = |k: &str| labels.get(k).and_then(|v| v.as_str());

        // Prefer an explicit Grove host; otherwise reuse the OrbStack domain.
        let (name, upstream) = if let Some(host) = get("grove.host") {
            let leaf = host.split('.').next().unwrap_or(host).to_string();
            let up = get("grove.upstream").map(str::to_string).or_else(|| {
                orbstack_domain(get("dev.orbstack.domains")).map(|d| format!("http://{d}"))
            });
            match up {
                Some(u) => (leaf, u),
                None => continue,
            }
        } else if let Some(domain) = orbstack_domain(get("dev.orbstack.domains")) {
            let leaf = domain.split('.').next().unwrap_or(&domain).to_string();
            (leaf, format!("http://{domain}"))
        } else {
            continue;
        };

        if !name.is_empty() && seen.insert(name.clone()) {
            out.push(DockerSite { name, upstream });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Whether the response headers declare chunked transfer encoding.
fn header_has_chunked(header: &[u8]) -> bool {
    String::from_utf8_lossy(header)
        .to_lowercase()
        .contains("transfer-encoding: chunked")
}

/// Minimal HTTP/1.1 chunked-body decoder.
fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(nl) = data.windows(2).position(|w| w == b"\r\n") {
        let size_str = String::from_utf8_lossy(&data[..nl]);
        let size =
            usize::from_str_radix(size_str.trim().split(';').next().unwrap_or("").trim(), 16)
                .unwrap_or(0);
        data = &data[nl + 2..];
        if size == 0 || data.len() < size {
            if size > 0 && !data.is_empty() {
                out.extend_from_slice(&data[..data.len().min(size)]);
            }
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
    out
}

/// First domain from a possibly comma-separated `dev.orbstack.domains` value.
fn orbstack_domain(value: Option<&str>) -> Option<String> {
    let v = value?.split(',').next().unwrap_or_default().trim();
    (!v.is_empty()).then(|| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn discover_live() {
        eprintln!("socket: {:?}", socket_path());
        let sites = discover().await;
        for s in &sites {
            eprintln!("DISCOVERED  {}.test  ->  {}", s.name, s.upstream);
        }
        eprintln!("total: {}", sites.len());
    }
}
