//! IPC listener: accepts CLI/GUI connections on a Unix socket and dispatches
//! one request per connection.
//!
//! # Who is allowed to talk to the daemon
//!
//! The daemon usually runs as **root** (it binds 80/443/53), and its request
//! surface is not advisory: `PhpInstall`, `ServiceInstall`, `DbDumpFile` and
//! `RestartDaemon` all make root do work on the caller's behalf. So the socket
//! is an authorization boundary, and it is enforced twice:
//!
//! 1. **File mode.** The socket is `0660`, owned by the user Grove serves — not
//!    the `0777` it used to be. The kernel turns away everyone else before a
//!    single byte is read.
//! 2. **Peer credentials.** Every accepted connection is checked with
//!    `SO_PEERCRED`/`LOCAL_PEERCRED` (via tokio's `peer_cred`) and rejected
//!    unless it comes from root or from that same user. Mode bits alone are a
//!    thin defence: they are lost the moment something re-creates the socket
//!    with a different umask, and they say nothing on a filesystem mounted
//!    without permission support.
//!
//! Being on the same machine is not authorization. Without this, any local
//! process — a compromised `npm`/`composer` postinstall hook, or a served PHP
//! app itself — could hand a root daemon arbitrary work.

use std::path::PathBuf;
use std::sync::Arc;

use grove_ipc::protocol::{Request, Response};
use grove_ipc::transport;

use crate::commands;
use crate::state::DaemonState;

/// The identities allowed to command the daemon.
///
/// Two independent signals, because neither is reliable alone:
///
/// - **`GROVE_RUN_USER_ID`**, written into the service unit by `grove install`
///   from `SUDO_UID`. Explicit and exact.
/// - **The owner of `$GROVE_HOME`**, for a daemon started by hand or by an
///   older service unit that predates the env var.
///
/// The owner alone would be a trap: `sudo grove install` calls
/// `CertificateAuthority::load_or_create` on the *user's* Grove home while still
/// running as root, so on a fresh machine root creates — and owns — that
/// directory. Trusting only the owner would then lock the user out of their own
/// daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PeerPolicy {
    /// The user the service unit says Grove runs on behalf of.
    run_as: Option<(u32, u32)>,
    /// The owner of `$GROVE_HOME`.
    owner: Option<(u32, u32)>,
    /// The uid the daemon itself runs as; always allowed to talk to itself.
    self_uid: u32,
}

impl PeerPolicy {
    #[cfg(unix)]
    fn discover(paths: &grove_core::paths::GrovePaths) -> Self {
        use std::os::unix::fs::MetadataExt;
        // A failed stat means "we cannot tell who we serve". Fall back to the
        // daemon's own uid rather than to something permissive: refusing a
        // legitimate client is recoverable, letting an illegitimate one through
        // is not.
        let owner = std::fs::metadata(paths.base())
            .ok()
            .map(|m| (m.uid(), m.gid()));
        Self {
            run_as: run_as_from_env(),
            owner,
            self_uid: current_uid(),
        }
    }

    /// Whether `uid` may issue requests.
    fn permits(&self, uid: u32) -> bool {
        // root is unconditionally allowed: it can already do everything the
        // daemon can, so rejecting it buys nothing and would break `sudo grove`.
        uid == 0
            || uid == self.self_uid
            || self.run_as.map(|(run, _)| uid == run).unwrap_or(false)
            || self.owner.map(|(owner, _)| uid == owner).unwrap_or(false)
    }

    /// Owner to apply to the socket file, when known.
    ///
    /// The explicit run-user wins: where the two disagree it is because root
    /// created `$GROVE_HOME`, and handing the socket to root would shut out the
    /// very client it exists for.
    fn socket_owner(&self) -> Option<(u32, u32)> {
        self.run_as.or(self.owner)
    }
}

/// The uid/gid pair the service unit recorded, if it did.
///
/// Numeric rather than a username so the daemon never has to resolve one — no
/// `getpwnam`, no NSS, nothing that can behave differently inside a sandbox.
#[cfg(unix)]
fn run_as_from_env() -> Option<(u32, u32)> {
    let uid: u32 = std::env::var("GROVE_RUN_USER_ID")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    // Never let a stray `GROVE_RUN_USER_ID=0` widen anything: root is already
    // allowed, and treating it as "the served user" would make the socket
    // root-owned and lock out the real one.
    if uid == 0 {
        return None;
    }
    let gid: u32 = std::env::var("GROVE_RUN_GROUP_ID")
        .ok()
        .and_then(|g| g.trim().parse().ok())
        .unwrap_or(uid);
    Some((uid, gid))
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // Safe: `geteuid` takes no arguments, touches no memory, and cannot fail.
    unsafe { libc_geteuid() }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

#[cfg(unix)]
pub async fn serve(socket: PathBuf, state: Arc<DaemonState>) -> anyhow::Result<()> {
    use tokio::net::UnixListener;

    // Remove a stale socket from a previous run.
    let _ = std::fs::remove_file(&socket);
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // sun_path is 104 bytes on macOS and 108 on Linux, NUL included. A long
    // GROVE_HOME hit this as a bare "path must be shorter than SUN_LEN" with no
    // path in it. Say which path, and what to do.
    const SUN_PATH_MAX: usize = if cfg!(target_os = "linux") { 107 } else { 103 };
    let len = socket.as_os_str().len();
    if len > SUN_PATH_MAX {
        anyhow::bail!(
            "IPC socket path is {len} bytes, and unix sockets allow at most {SUN_PATH_MAX}: {}. \
             Use a shorter GROVE_HOME.",
            socket.display()
        );
    }
    let policy = PeerPolicy::discover(&state.paths);
    let listener = UnixListener::bind(&socket)?;
    restrict_socket(&socket, &policy);
    tracing::info!(
        socket = %socket.display(),
        owner = ?policy.socket_owner().map(|(uid, _)| uid),
        "IPC listening"
    );

    let shutdown = state.shutdown.clone();
    // Like the proxy's accept loop: a transient accept error (out of file
    // descriptors, a connection reset in the backlog) backs off and retries.
    // It used to `?` out of this loop, which ended the daemon and skipped the
    // socket and pidfile cleanup below.
    let mut backoff = std::time::Duration::from_millis(5);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let stream = match accepted {
                    Ok((stream, _addr)) => {
                        backoff = std::time::Duration::from_millis(5);
                        stream
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, backoff_ms = backoff.as_millis(), "IPC accept failed");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(std::time::Duration::from_secs(1));
                        continue;
                    }
                };
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, state, policy).await {
                        tracing::debug!(error = %e, "IPC connection error");
                    }
                });
            }
            _ = shutdown.notified() => {
                tracing::info!("shutdown requested, stopping IPC listener");
                break;
            }
        }
    }
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

/// Hand the socket to the user Grove serves and take it away from everyone else.
///
/// Best-effort by design: on a filesystem that cannot express this, the
/// peer-credential check in [`handle_conn`] is what actually holds the line, and
/// a daemon that refused to start here would be a worse outcome than one that
/// starts with the weaker of its two defences.
#[cfg(unix)]
fn restrict_socket(socket: &std::path::Path, policy: &PeerPolicy) {
    use std::os::unix::fs::PermissionsExt;
    if let Some((uid, gid)) = policy.socket_owner() {
        if let Err(e) = std::os::unix::fs::chown(socket, Some(uid), Some(gid)) {
            tracing::warn!(error = %e, "could not set IPC socket owner");
        }
    }
    if let Err(e) = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660)) {
        tracing::warn!(error = %e, "could not restrict IPC socket permissions");
    }
}

#[cfg(unix)]
async fn handle_conn(
    stream: tokio::net::UnixStream,
    state: Arc<DaemonState>,
    policy: PeerPolicy,
) -> anyhow::Result<()> {
    // Authorize before reading anything: the request is what says "install this
    // runtime" or "dump a database to this path", so it must not reach
    // `dispatch` from a caller we have not vetted.
    let peer = stream.peer_cred()?;
    let authorized = policy.permits(peer.uid());

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = transport::buf_reader(read_half);

    if !authorized {
        tracing::warn!(
            uid = peer.uid(),
            pid = ?peer.pid(),
            "rejected IPC request from an unauthorized user"
        );
        // Answer rather than hanging up. A refusal that looks like a crash sends
        // people debugging the wrong thing; this is a local socket, and "you are
        // the wrong user" tells an attacker nothing they could not already see
        // from the socket's mode bits.
        let response = Response::err(format!(
            "not authorized: uid {} may not command the Grove daemon",
            peer.uid()
        ));
        let _ = transport::write_message(&mut write_half, &response).await;
        return Ok(());
    }

    // One request/response per connection keeps the protocol trivial.
    let request: Request = match transport::read_message(&mut reader).await {
        Ok(r) => r,
        Err(transport::TransportError::Closed) => return Ok(()),
        // A request this daemon cannot parse is almost always a newer CLI
        // talking to an older daemon. It used to drop the connection, which the
        // CLI reported as "connection closed before a full message was
        // received" — true, and useless. Say what is going on.
        Err(transport::TransportError::Json(e)) => {
            let response = Response::err(format!(
                "the daemon (version {}) did not understand this request: {e}. \
                 If Grove was upgraded, run `grove restart` so the daemon matches the CLI.",
                env!("CARGO_PKG_VERSION")
            ));
            let _ = transport::write_message(&mut write_half, &response).await;
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    let response: Response = commands::dispatch(&state, request).await;
    transport::write_message(&mut write_half, &response).await?;
    Ok(())
}

#[cfg(not(unix))]
pub async fn serve(_socket: PathBuf, _state: Arc<DaemonState>) -> anyhow::Result<()> {
    anyhow::bail!("named-pipe IPC not yet implemented on this platform");
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// `set_var`/`remove_var` are process-global, and cargo runs tests in
    /// parallel threads of one process. Any test that touches the run-user env
    /// takes this lock, so they cannot read each other's writes.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn policy(owner: Option<(u32, u32)>, self_uid: u32) -> PeerPolicy {
        PeerPolicy {
            run_as: None,
            owner,
            self_uid,
        }
    }

    fn policy_with_run_as(
        run_as: Option<(u32, u32)>,
        owner: Option<(u32, u32)>,
        self_uid: u32,
    ) -> PeerPolicy {
        PeerPolicy {
            run_as,
            owner,
            self_uid,
        }
    }

    #[test]
    fn root_is_always_allowed() {
        // Rejecting root would buy nothing (it can do everything the daemon
        // can) and would break `sudo grove`.
        assert!(policy(Some((501, 20)), 0).permits(0));
        assert!(policy(None, 501).permits(0));
    }

    #[test]
    fn the_served_user_is_allowed() {
        assert!(policy(Some((501, 20)), 0).permits(501));
    }

    #[test]
    fn the_daemons_own_uid_is_allowed() {
        // Rootless dev: the daemon runs as the user on high ports.
        assert!(policy(None, 501).permits(501));
    }

    #[test]
    fn every_other_local_user_is_refused() {
        let p = policy(Some((501, 20)), 0);
        for uid in [1, 70, 502, 999, 65534] {
            assert!(!p.permits(uid), "uid {uid} should be refused");
        }
    }

    /// The interesting failure: if we cannot tell who we serve, the fallback
    /// must not be "everyone".
    #[test]
    fn unknown_owner_does_not_open_the_door() {
        let p = policy(None, 0);
        assert!(p.permits(0));
        for uid in [1, 501, 502] {
            assert!(!p.permits(uid), "uid {uid} should be refused");
        }
    }

    /// The regression this policy nearly shipped with. `sudo grove install`
    /// runs `CertificateAuthority::load_or_create` on the *user's* Grove home
    /// while still root, so on a fresh machine that directory is root-owned.
    /// Deriving trust from the owner alone would lock the user out of their own
    /// daemon; the recorded run-user is what saves them.
    #[test]
    fn a_root_owned_grove_home_still_admits_the_served_user() {
        let root_owned = policy(Some((0, 0)), 0);
        assert!(
            !root_owned.permits(501),
            "owner-only trust is what breaks here"
        );

        let with_run_as = policy_with_run_as(Some((501, 20)), Some((0, 0)), 0);
        assert!(with_run_as.permits(501));
        assert!(with_run_as.permits(0));
        assert!(!with_run_as.permits(502));
    }

    /// Where the two signals disagree the socket must follow the run-user, or it
    /// ends up root-owned and unreachable by the client it exists for.
    #[test]
    fn socket_owner_prefers_the_recorded_run_user() {
        let p = policy_with_run_as(Some((501, 20)), Some((0, 0)), 0);
        assert_eq!(p.socket_owner(), Some((501, 20)));

        // With nothing recorded, fall back to the directory owner.
        let p = policy(Some((501, 20)), 0);
        assert_eq!(p.socket_owner(), Some((501, 20)));
    }

    #[test]
    fn a_recorded_root_run_user_is_ignored() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        // `GROVE_RUN_USER_ID=0` must not make the socket root-owned.
        std::env::set_var("GROVE_RUN_USER_ID", "0");
        assert_eq!(run_as_from_env(), None);

        std::env::set_var("GROVE_RUN_USER_ID", "501");
        std::env::set_var("GROVE_RUN_GROUP_ID", "20");
        assert_eq!(run_as_from_env(), Some((501, 20)));

        // A gid we cannot read falls back to the uid rather than to 0 (wheel).
        std::env::remove_var("GROVE_RUN_GROUP_ID");
        assert_eq!(run_as_from_env(), Some((501, 501)));

        std::env::set_var("GROVE_RUN_USER_ID", "not-a-number");
        assert_eq!(run_as_from_env(), None);

        std::env::remove_var("GROVE_RUN_USER_ID");
        std::env::remove_var("GROVE_RUN_GROUP_ID");
        assert_eq!(run_as_from_env(), None);
    }

    /// `discover` against a real directory: the user running the tests owns the
    /// temp dir, so they must be permitted. Guards against a policy that is
    /// airtight in unit tests and locks everyone out in practice.
    #[test]
    fn discover_permits_the_owner_of_a_real_grove_home() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("GROVE_RUN_USER_ID");
        let dir = std::env::temp_dir().join(format!("grove-ipc-discover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = grove_core::paths::GrovePaths::with_base(&dir);

        let p = PeerPolicy::discover(&paths);
        assert!(
            p.permits(current_uid()),
            "the owner of $GROVE_HOME must be allowed: {p:?}"
        );
        assert!(p.permits(0), "root must be allowed");
        assert!(p.socket_owner().is_some());

        let _ = std::fs::remove_dir(&dir);
    }

    /// The kernel-enforced half, against a real socket: mode `0660`, and in
    /// particular **not** world-accessible. The old `0777` is the bug this
    /// branch exists to remove, so assert on the actual bits rather than
    /// trusting that the call was made.
    #[tokio::test]
    async fn the_socket_is_not_world_accessible() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("grove-ipc-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("mode.sock");
        let _ = std::fs::remove_file(&socket);

        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        // Own ids, so the chown is a valid no-op for a non-root test run.
        let policy = policy_with_run_as(None, Some((current_uid(), current_gid())), current_uid());
        restrict_socket(&socket, &policy);

        let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o660, "socket mode is {mode:o}, expected 660");
        assert_eq!(mode & 0o007, 0, "socket must not be reachable by others");

        drop(_listener);
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_dir(&dir);
    }

    fn current_gid() -> u32 {
        // Safe: `getegid` takes no arguments, touches no memory, and cannot fail.
        unsafe { libc_getegid() }
    }

    extern "C" {
        #[link_name = "getegid"]
        fn libc_getegid() -> u32;
    }

    #[tokio::test]
    async fn an_unauthorized_peer_gets_an_error_and_no_dispatch() {
        // End-to-end over a real socket pair: the connection is answered with an
        // error and the request is never read, let alone dispatched.
        let dir = std::env::temp_dir().join(format!(
            "grove-ipc-authz-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("test.sock");
        let _ = std::fs::remove_file(&socket);

        // A policy that trusts nobody but root. As root there is no
        // unauthorized peer to be — root is always permitted, by design — so
        // under a root test run (a container) this proves nothing and skips.
        if grove_core::privdrop::running_as_root() {
            eprintln!("skipped: runs as root, and root is always authorized");
            return;
        }
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let deny_all = policy(Some((0, 0)), 0);

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let peer = stream.peer_cred().unwrap();
            let authorized = deny_all.permits(peer.uid());
            let (_r, mut w) = stream.into_split();
            if !authorized {
                let resp = Response::err(format!(
                    "not authorized: uid {} may not command the Grove daemon",
                    peer.uid()
                ));
                transport::write_message(&mut w, &resp).await.unwrap();
            }
            authorized
        });

        let client = tokio::net::UnixStream::connect(&socket).await.unwrap();
        let (cr, _cw) = client.into_split();
        let mut reader = transport::buf_reader(cr);
        let resp: Response = transport::read_message(&mut reader).await.unwrap();

        let authorized = server.await.unwrap();
        assert!(!authorized, "this test must run as a non-root user");
        assert!(!resp.ok);
        assert!(
            resp.error
                .as_deref()
                .unwrap_or_default()
                .contains("not authorized"),
            "{:?}",
            resp.error
        );

        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_dir(&dir);
    }
}
