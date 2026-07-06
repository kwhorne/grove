//! Auto-discover Docker / OrbStack containers and expose them as `<name>.test`.
//!
//! Discovery order per container/project:
//!   1. A `grove.host` / `dev.orbstack.domains` label (Grove reuses the routing).
//!   2. A `docker compose` project with a published web port (no label needed) —
//!      Grove routes `<project>.test` to `127.0.0.1:<published-port>`.
//!
//! Grove terminates TLS with its trusted CA and reverse-proxies, so containers
//! become first-class `*.test` sites with local HTTPS. Containers can also be
//! started / stopped from the GUI.
//!
//! We speak the Docker Engine API over its unix socket with tiny HTTP/1.0
//! requests (no extra deps) and a hard timeout so it can never hang the daemon.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// A container Grove tracks as a `<name>.<tld>` site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerContainer {
    /// Site leaf name (`inside2` → `inside2.test`).
    pub name: String,
    /// Container id (for start/stop/restart).
    pub id: String,
    /// Upstream to proxy to when running (`None` when it can't be routed yet).
    pub upstream: Option<String>,
    pub running: bool,
}

const WEB_SERVICES: [&str; 9] = [
    "nginx", "web", "app", "caddy", "httpd", "frontend", "server", "octane", "http",
];
const WEB_PORTS: [u64; 9] = [80, 8080, 8000, 3000, 5173, 4000, 5000, 8081, 443];

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

/// GET a Docker API path and return the JSON body, or `None`.
async fn get_json(path: &str) -> Option<serde_json::Value> {
    let sock = socket_path()?;
    let mut stream = UnixStream::connect(&sock).await.ok()?;
    let req = format!(
        "GET {path} HTTP/1.0\r\nHost: localhost\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.ok()?;
    let pos = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let header = &buf[..pos];
    let raw = &buf[pos + 4..];
    let decoded;
    let body: &[u8] = if header_has_chunked(header) {
        decoded = dechunk(raw);
        &decoded
    } else {
        raw
    };
    serde_json::from_slice(body).ok()
}

/// POST to a Docker API path (used for start/stop/restart). Returns success.
async fn post(path: &str) -> bool {
    let Some(sock) = socket_path() else {
        return false;
    };
    let Ok(mut stream) = UnixStream::connect(&sock).await else {
        return false;
    };
    let req = format!(
        "POST {path} HTTP/1.0\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(req.as_bytes()).await.is_err() {
        return false;
    }
    let mut buf = Vec::new();
    if stream.read_to_end(&mut buf).await.is_err() {
        return false;
    }
    let head = String::from_utf8_lossy(&buf[..buf.len().min(64)]);
    // 204 No Content = success; 304 = already in that state (also fine).
    head.contains(" 204") || head.contains(" 304")
}

/// Start / stop / restart a container by id.
pub async fn control(id: &str, action: &str) -> Result<(), String> {
    let action = match action {
        "start" | "stop" | "restart" => action,
        other => return Err(format!("unknown action: {other}")),
    };
    let path = format!("/containers/{id}/{action}");
    match tokio::time::timeout(Duration::from_secs(20), post(&path)).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("docker {action} failed")),
        Err(_) => Err(format!("docker {action} timed out")),
    }
}

/// Discover container-backed sites (running and stopped). Empty when Docker
/// isn't present, so it's safe to call unconditionally.
pub async fn discover() -> Vec<DockerContainer> {
    tokio::time::timeout(Duration::from_secs(3), discover_inner())
        .await
        .unwrap_or_default()
}

async fn discover_inner() -> Vec<DockerContainer> {
    let Some(list) = get_json("/containers/json?all=1").await else {
        return Vec::new();
    };
    let Some(arr) = list.as_array() else {
        return Vec::new();
    };

    let mut out: Vec<DockerContainer> = Vec::new();
    let mut names = HashSet::new();
    // Compose candidates (project -> best web container), for the label-less path.
    let mut compose: HashMap<String, DockerContainer> = HashMap::new();
    let mut compose_score: HashMap<String, i32> = HashMap::new();

    for c in arr {
        let id = c
            .get("Id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let running = c.get("State").and_then(|s| s.as_str()) == Some("running");
        let labels = c.get("Labels").and_then(|l| l.as_object());
        let get = |k: &str| {
            labels
                .and_then(|l| l.get(k))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        };

        // 1. Explicit label wins (running or stopped).
        let labelled = if let Some(host) = get("grove.host") {
            let leaf = host.split('.').next().unwrap_or(host).to_string();
            let up = get("grove.upstream").map(str::to_string).or_else(|| {
                orbstack_domain(get("dev.orbstack.domains")).map(|d| format!("http://{d}"))
            });
            Some((leaf, up))
        } else {
            orbstack_domain(get("dev.orbstack.domains")).map(|domain| {
                let leaf = domain.split('.').next().unwrap_or(&domain).to_string();
                (leaf, Some(format!("http://{domain}")))
            })
        };

        if let Some((name, upstream)) = labelled {
            if !name.is_empty() && names.insert(name.clone()) {
                out.push(DockerContainer {
                    name,
                    id,
                    upstream,
                    running,
                });
            }
            continue;
        }

        // 2. Compose fallback (running only — needs a live published port).
        if !running {
            continue;
        }
        let Some(project) = get("com.docker.compose.project") else {
            continue;
        };
        let service = get("com.docker.compose.service").unwrap_or("");
        let Some((port, priv_port)) = published_web_port(c) else {
            continue;
        };
        // Score: prefer web-named services and canonical web ports.
        let mut score = 0;
        if WEB_SERVICES.iter().any(|w| service.contains(w)) {
            score += 100;
        }
        if WEB_PORTS.contains(&priv_port) {
            score += 10;
        }
        if score >= *compose_score.get(project).unwrap_or(&-1) {
            compose_score.insert(project.to_string(), score);
            compose.insert(
                project.to_string(),
                DockerContainer {
                    name: project.to_string(),
                    id: id.clone(),
                    upstream: Some(format!("http://127.0.0.1:{port}")),
                    running: true,
                },
            );
        }
    }

    for (project, cont) in compose {
        if names.insert(project) {
            out.push(cont);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Pick a published web port for a container: returns (public, private).
fn published_web_port(c: &serde_json::Value) -> Option<(u64, u64)> {
    let ports = c.get("Ports").and_then(|p| p.as_array())?;
    let published: Vec<(u64, u64)> = ports
        .iter()
        .filter(|p| p.get("Type").and_then(|t| t.as_str()) == Some("tcp"))
        .filter_map(|p| {
            let public = p.get("PublicPort").and_then(|v| v.as_u64())?;
            let private = p
                .get("PrivatePort")
                .and_then(|v| v.as_u64())
                .unwrap_or(public);
            Some((public, private))
        })
        .collect();
    // Prefer a canonical web private port; else the first published one.
    published
        .iter()
        .find(|(_, priv_p)| WEB_PORTS.contains(priv_p))
        .or_else(|| published.first())
        .copied()
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
        for c in discover().await {
            eprintln!(
                "DISCOVERED  {}.test  running={}  -> {:?}  ({})",
                c.name, c.running, c.upstream, c.id
            );
        }
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;
    #[tokio::test]
    #[ignore]
    async fn control_bogus_id_errors() {
        // POST plumbing works: a non-existent container id yields an error, not a hang.
        let r = control("grove-does-not-exist-zzz", "restart").await;
        eprintln!("control result: {r:?}");
        assert!(r.is_err());
    }
    #[tokio::test]
    async fn control_rejects_unknown_action() {
        assert!(control("x", "frobnicate").await.is_err());
    }
}
