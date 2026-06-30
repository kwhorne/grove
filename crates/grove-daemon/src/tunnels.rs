//! Daemon-managed public tunnels. The GUI/CLI start and stop tunnels through
//! IPC; the daemon owns the long-running tunnel clients, tracks their public
//! URLs, and keeps a ring buffer of recent requests for the inspector.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use grove_ipc::protocol::{TunnelRequestEntry, TunnelStatus};
use grove_tunnel::{now_ms, Recorder, RequestRecord, ShareConfig};

/// Keep at most this many requests per tunnel for the inspector.
const MAX_RECORDS: usize = 250;

/// Result delivered once the tunnel handshake completes (or fails).
type ReadyResult = Result<(String, String), String>;
/// Shared one-shot slot the on_ready callback and error path race to fill.
type ReadySlot = Arc<StdMutex<Option<oneshot::Sender<ReadyResult>>>>;

struct ActiveTunnel {
    public_url: String,
    public_host: String,
    started_at_ms: u64,
    handle: JoinHandle<()>,
    records: Arc<StdMutex<VecDeque<RequestRecord>>>,
    count: Arc<AtomicU64>,
}

impl Drop for ActiveTunnel {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Supervises every active tunnel, keyed by site name.
#[derive(Default)]
pub struct TunnelManager {
    inner: Mutex<HashMap<String, ActiveTunnel>>,
}

impl TunnelManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a tunnel for `site`. Blocks until the public URL is assigned.
    pub async fn start(&self, site: String, cfg: ShareConfig) -> anyhow::Result<TunnelStatus> {
        {
            let map = self.inner.lock().await;
            if map.contains_key(&site) {
                anyhow::bail!("already sharing {site}");
            }
        }

        let records = Arc::new(StdMutex::new(VecDeque::new()));
        let count = Arc::new(AtomicU64::new(0));

        let rec_records = records.clone();
        let rec_count = count.clone();
        let recorder: Recorder = Arc::new(move |r: RequestRecord| {
            rec_count.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut q) = rec_records.lock() {
                if q.len() >= MAX_RECORDS {
                    q.pop_front();
                }
                q.push_back(r);
            }
        });

        // A slot the on_ready callback fills, and that the post-run error path
        // can fall back on if the handshake fails before readiness.
        let (tx, rx) = oneshot::channel();
        let slot: ReadySlot = Arc::new(StdMutex::new(Some(tx)));

        let ready_slot = slot.clone();
        let handle = tokio::spawn(async move {
            let res = grove_tunnel::share(cfg, Some(recorder), move |host, url| {
                if let Some(tx) = ready_slot.lock().ok().and_then(|mut s| s.take()) {
                    let _ = tx.send(Ok((host.to_string(), url.to_string())));
                }
            })
            .await;
            if let Err(e) = res {
                if let Some(tx) = slot.lock().ok().and_then(|mut s| s.take()) {
                    let _ = tx.send(Err(e.to_string()));
                }
            }
        });

        let started_at_ms = now_ms();
        let outcome = tokio::time::timeout(Duration::from_secs(12), rx).await;
        let (public_host, public_url) = match outcome {
            Ok(Ok(Ok(pair))) => pair,
            Ok(Ok(Err(msg))) => {
                handle.abort();
                anyhow::bail!("{msg}");
            }
            _ => {
                handle.abort();
                anyhow::bail!("tunnel did not connect in time");
            }
        };

        let status = TunnelStatus {
            site: site.clone(),
            public_url: public_url.clone(),
            public_host: public_host.clone(),
            started_at_ms,
            request_count: 0,
        };

        self.inner.lock().await.insert(
            site,
            ActiveTunnel {
                public_url,
                public_host,
                started_at_ms,
                handle,
                records,
                count,
            },
        );
        Ok(status)
    }

    /// Stop sharing `site`.
    pub async fn stop(&self, site: &str) -> anyhow::Result<()> {
        match self.inner.lock().await.remove(site) {
            Some(_) => Ok(()), // Drop aborts the task.
            None => anyhow::bail!("{site} is not being shared"),
        }
    }

    /// Snapshot of every active tunnel.
    pub async fn list(&self) -> Vec<TunnelStatus> {
        let map = self.inner.lock().await;
        let mut out: Vec<TunnelStatus> = map
            .iter()
            .map(|(site, t)| TunnelStatus {
                site: site.clone(),
                public_url: t.public_url.clone(),
                public_host: t.public_host.clone(),
                started_at_ms: t.started_at_ms,
                request_count: t.count.load(Ordering::Relaxed),
            })
            .collect();
        out.sort_by(|a, b| a.site.cmp(&b.site));
        out
    }

    /// Recent requests across all tunnels, or just one site, newest first.
    pub async fn requests(&self, site: Option<&str>) -> Vec<TunnelRequestEntry> {
        let map = self.inner.lock().await;
        let mut entries: Vec<TunnelRequestEntry> = Vec::new();
        for (name, t) in map.iter() {
            if let Some(filter) = site {
                if filter != name {
                    continue;
                }
            }
            if let Ok(q) = t.records.lock() {
                for r in q.iter() {
                    entries.push(TunnelRequestEntry {
                        site: name.clone(),
                        at_unix_ms: r.at_unix_ms,
                        method: r.method.clone(),
                        path: r.path.clone(),
                        status: r.status,
                        duration_ms: r.duration_ms,
                    });
                }
            }
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.at_unix_ms));
        entries.truncate(MAX_RECORDS);
        entries
    }
}
