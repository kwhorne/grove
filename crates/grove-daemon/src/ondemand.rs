//! Databases that run only while something is connected.
//!
//! An installed MySQL sits at 700 MB or more whether anything talks to it or
//! not, and it has been doing that since the machine booted. For a service in
//! on-demand mode the daemon holds the public port instead — 3306, 5432,
//! 6379 — and the real server does not run at all until the first connection
//! arrives. That connection is held for the fraction of a second the server
//! takes to come up (measured at 0.35 s for MySQL), then spliced through to
//! it. When nothing has been connected for the idle period, the server is
//! stopped cleanly and the port goes back to costing nothing.
//!
//! Every client goes through the front, and that is what makes the idle
//! decision sound: the count of open connections here *is* the count of
//! clients. The server's own unix socket is moved to a private name in this
//! mode for the same reason — a client on it would go uncounted and be cut off
//! when the server went idle — and none of the projects on the machine this
//! was built on connect that way.
//!
//! The PHP-FPM pools have worked like this from the start. This is the same
//! idea one layer further in.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use grove_services::ServiceManager;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// The public listener for every service in on-demand mode.
#[derive(Default)]
pub struct Fronts {
    open: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl Fronts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take over `key`'s public port and start serving it on demand.
    ///
    /// A server already running *on* that port — the always-on mode this is
    /// replacing — is stopped cleanly first, so the port is free to bind.
    pub async fn open(&self, services: Arc<ServiceManager>, key: &str) -> anyhow::Result<u16> {
        let port = services
            .port_of(key)
            .ok_or_else(|| anyhow::anyhow!("unknown service {key}"))?;
        let mut open = self.open.lock().await;
        if open.contains_key(key) {
            return Ok(port);
        }
        if services.upstream_port(key).is_none() && running(&services, key) {
            let (s, k) = (services.clone(), key.to_string());
            tokio::task::spawn_blocking(move || s.suspend(&k)).await??;
        }
        let listener = bind(port).await?;
        tracing::info!(
            service = key,
            port,
            "on demand: holding the port; the server starts on first connection"
        );
        let task = tokio::spawn(serve(services, key.to_string(), listener));
        open.insert(key.to_string(), task);
        Ok(port)
    }

    /// Give the port back. Connections already spliced through finish on
    /// their own; nothing new is accepted.
    pub async fn close(&self, key: &str) {
        if let Some(task) = self.open.lock().await.remove(key) {
            task.abort();
            // The listener is dropped with the task; wait for that, so a
            // caller about to bind the same port does not race it.
            let _ = task.await;
        }
    }

    pub async fn is_open(&self, key: &str) -> bool {
        self.open.lock().await.contains_key(key)
    }
}

fn running(services: &ServiceManager, key: &str) -> bool {
    services
        .status_all()
        .into_iter()
        .any(|s| s.key == key && s.running)
}

/// Bind the public port, retrying briefly: a server that has just been told
/// to stop can hold it for a moment after it says it is done.
async fn bind(port: u16) -> anyhow::Result<TcpListener> {
    let mut last = None;
    for _ in 0..40 {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => return Ok(l),
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    Err(anyhow::anyhow!(
        "could not take port {port} for on-demand start: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// How often to look at whether an idle server should stop.
fn tick_for(idle: Duration) -> Duration {
    (idle / 5).clamp(Duration::from_secs(1), Duration::from_secs(15))
}

/// Accept on the public port, and stop the server when it has sat idle.
async fn serve(services: Arc<ServiceManager>, key: String, listener: TcpListener) {
    let active = Arc::new(AtomicUsize::new(0));
    let last_seen = Arc::new(std::sync::Mutex::new(Instant::now()));
    // Held while a connection finds (or starts) the server and dials it, and
    // while the idle check decides and stops. So a connection can never be
    // handed a server that is being stopped under it.
    let gate = Arc::new(Mutex::new(()));

    loop {
        let idle = services
            .on_demand_idle(&key)
            .unwrap_or(Duration::from_secs(600));
        tokio::select! {
            accepted = listener.accept() => {
                let (client, _) = match accepted {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(service = %key, error = %e, "on demand: accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                active.fetch_add(1, Ordering::SeqCst);
                touch(&last_seen);
                let (services, key, active, last_seen, gate) =
                    (services.clone(), key.clone(), active.clone(), last_seen.clone(), gate.clone());
                tokio::spawn(async move {
                    if let Err(e) = splice(&services, &key, client, &gate).await {
                        tracing::warn!(service = %key, error = %e, "on demand: connection not served");
                    }
                    touch(&last_seen);
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            _ = tokio::time::sleep(tick_for(idle)) => {
                let quiet_for = last_seen.lock().map(|t| t.elapsed()).unwrap_or_default();
                if active.load(Ordering::SeqCst) != 0 || quiet_for < idle {
                    continue;
                }
                let _held = gate.lock().await;
                // Re-checked under the gate: a connection that arrived while we
                // waited for it has already counted itself.
                if active.load(Ordering::SeqCst) != 0 || services.upstream_port(&key).is_none() {
                    continue;
                }
                let (s, k) = (services.clone(), key.clone());
                match tokio::task::spawn_blocking(move || s.suspend(&k)).await {
                    Ok(Ok(())) => tracing::info!(
                        service = %key,
                        idle_secs = quiet_for.as_secs(),
                        "on demand: nothing connected, stopped the server"
                    ),
                    Ok(Err(e)) => tracing::warn!(service = %key, error = %e, "on demand: could not stop idle server"),
                    Err(e) => tracing::warn!(service = %key, error = %e, "on demand: stop task failed"),
                }
            }
        }
    }
}

fn touch(t: &std::sync::Mutex<Instant>) {
    if let Ok(mut t) = t.lock() {
        *t = Instant::now();
    }
}

/// Find the server — starting it if it is not up — and join the client to it.
async fn splice(
    services: &Arc<ServiceManager>,
    key: &str,
    mut client: TcpStream,
    gate: &Mutex<()>,
) -> anyhow::Result<()> {
    let mut server = {
        let _held = gate.lock().await;
        let port = match services.upstream_port(key) {
            Some(p) => p,
            None => {
                let started = Instant::now();
                let (s, k) = (services.clone(), key.to_string());
                tokio::task::spawn_blocking(move || s.start(&k)).await??;
                tracing::info!(
                    service = key,
                    ms = started.elapsed().as_millis() as u64,
                    "on demand: started for the first connection"
                );
                services
                    .upstream_port(key)
                    .ok_or_else(|| anyhow::anyhow!("{key} started but reported no port"))?
            }
        };
        TcpStream::connect(("127.0.0.1", port)).await?
    };
    let _ = client.set_nodelay(true);
    let _ = server.set_nodelay(true);
    // A peer closing is how every connection ends; only report real faults.
    match tokio::io::copy_bidirectional(&mut client, &mut server).await {
        Ok(_) => Ok(()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The idle check runs often enough to be timely for a short timeout and
    /// rarely enough to cost nothing for a long one.
    #[test]
    fn the_idle_check_scales_with_the_timeout() {
        assert_eq!(tick_for(Duration::from_secs(3)), Duration::from_secs(1));
        assert_eq!(tick_for(Duration::from_secs(30)), Duration::from_secs(6));
        assert_eq!(tick_for(Duration::from_secs(600)), Duration::from_secs(15));
    }

    /// The splice is plain bytes both ways: what a client writes reaches the
    /// server, and the server's answer comes back, through the same code the
    /// front uses once the server is found.
    #[tokio::test]
    async fn bytes_are_carried_both_ways() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = server.accept().await.unwrap();
            // Server speaks first, as MySQL's greeting does.
            s.write_all(b"hello").await.unwrap();
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
        });
        let front = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let front_addr = front.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut c, _) = front.accept().await.unwrap();
            let mut s = TcpStream::connect(server_addr).await.unwrap();
            let _ = tokio::io::copy_bidirectional(&mut c, &mut s).await;
        });
        let mut client = TcpStream::connect(front_addr).await.unwrap();
        let mut greet = [0u8; 5];
        client.read_exact(&mut greet).await.unwrap();
        assert_eq!(&greet, b"hello");
        client.write_all(b"ping").await.unwrap();
        let mut echo = [0u8; 4];
        client.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"ping");
    }
}
