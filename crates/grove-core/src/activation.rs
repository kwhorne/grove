//! Sockets the service manager bound for us.
//!
//! Grove's daemon runs as root for one reason above all others: ports 53, 80
//! and 443 are below 1024, and only root may bind them. But binding is not the
//! same as serving. Both launchd and systemd will bind a port *themselves*,
//! while they are root, and hand the already-listening file descriptor to a
//! process that never had privilege at all. The daemon then serves on a
//! privileged port without ever having been privileged.
//!
//! This module is the receiving end. It answers one question — "did the
//! supervisor already bind the port I am about to bind?" — and hands back an
//! owned socket when the answer is yes.
//!
//! ## Why it never insists
//!
//! Grove updates itself: a new app bundle appears, the CLI symlink follows it,
//! and the daemon restarts. Nothing rewrites the launchd plist, and nothing
//! re-runs `grove install`. So a daemon that *required* delivered sockets would
//! break every machine it landed on the moment it shipped. Every caller here
//! gets an `Option`, and the caller binds for itself when it is `None`. The new
//! path stays dead until the unit is rewritten, and rewriting the unit is an
//! explicit, visible `sudo grove install`.
//!
//! ## Matching is by port
//!
//! The supervisor's configuration names ports, not roles, and the fds arrive as
//! an unordered set. So a caller asks for "the TCP listener on 443" and gets it
//! or does not. Address is deliberately not part of the match: the unit decides
//! whether HTTP listens on every interface or on loopback, and a daemon that
//! second-guessed that would be overriding the administrator who wrote it. The
//! address that was actually bound is logged, so a mismatch is visible rather
//! than silent.

#[cfg(unix)]
use std::os::unix::io::{FromRawFd, RawFd};

/// What kind of socket arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A listening TCP socket.
    Stream,
    /// A bound UDP socket.
    Datagram,
}

/// One socket the supervisor bound and handed over.
#[derive(Debug, Clone, Copy)]
struct Handed {
    #[cfg(unix)]
    fd: RawFd,
    kind: Kind,
    port: u16,
}

/// The set of sockets this process was started with.
///
/// Empty when nothing was delivered, which is the ordinary case for a daemon
/// started from a shell, from an old unit, or by `grove start`.
#[derive(Debug, Default)]
pub struct Inherited {
    sockets: Vec<Handed>,
}

impl Inherited {
    /// Collect whatever the supervisor delivered.
    ///
    /// Never fails: a process that was not started by launchd or systemd, or
    /// was started by a unit with no socket configuration, gets an empty set
    /// and binds everything itself.
    pub fn from_environment() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::classify(macos::delivered_fds())
        }
        #[cfg(target_os = "linux")]
        {
            Self::classify(linux::delivered_fds())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Self::default()
        }
    }

    /// Is there nothing to hand out?
    pub fn is_empty(&self) -> bool {
        self.sockets.is_empty()
    }

    /// How many sockets arrived.
    pub fn len(&self) -> usize {
        self.sockets.len()
    }

    /// The listening TCP socket on `port`, if one was delivered.
    ///
    /// Taken, not borrowed: the descriptor is handed out exactly once, so two
    /// callers cannot end up accepting on the same listener.
    #[cfg(unix)]
    pub fn take_tcp(&mut self, port: u16) -> Option<std::net::TcpListener> {
        let handed = self.take(Kind::Stream, port)?;
        // SAFETY: the fd came from the supervisor, has been claimed here so
        // nothing else owns it, and `SO_TYPE` said it is a stream socket.
        Some(unsafe { std::net::TcpListener::from_raw_fd(handed.fd) })
    }

    /// The bound UDP socket on `port`, if one was delivered.
    #[cfg(unix)]
    pub fn take_udp(&mut self, port: u16) -> Option<std::net::UdpSocket> {
        let handed = self.take(Kind::Datagram, port)?;
        // SAFETY: as `take_tcp`, with `SO_TYPE` reporting a datagram socket.
        Some(unsafe { std::net::UdpSocket::from_raw_fd(handed.fd) })
    }

    /// One line naming what arrived, for the startup log.
    pub fn describe(&self) -> String {
        if self.sockets.is_empty() {
            return "none".to_string();
        }
        let mut parts: Vec<String> = self
            .sockets
            .iter()
            .map(|s| {
                let proto = match s.kind {
                    Kind::Stream => "tcp",
                    Kind::Datagram => "udp",
                };
                format!("{proto}/{}", s.port)
            })
            .collect();
        parts.sort();
        parts.join(", ")
    }

    fn take(&mut self, kind: Kind, port: u16) -> Option<Handed> {
        let at = self
            .sockets
            .iter()
            .position(|s| s.kind == kind && s.port == port)?;
        Some(self.sockets.remove(at))
    }

    /// Turn raw descriptors into a set we can answer questions about.
    ///
    /// Split out from [`from_environment`] so the classification — which is
    /// where the interesting mistakes live — can be tested against sockets this
    /// process bound itself, without a supervisor.
    #[cfg(unix)]
    pub fn classify(fds: Vec<RawFd>) -> Self {
        let sockets = fds
            .into_iter()
            .filter_map(|fd| {
                let kind = sys::socket_kind(fd)?;
                let port = sys::local_port(fd)?;
                // A descriptor we keep must not reach the things the daemon
                // spawns. php-fpm holding an inherited copy of port 80 would
                // keep the port alive after the daemon that owns it is gone.
                sys::set_close_on_exec(fd);
                // tokio requires a non-blocking socket; the supervisor makes no
                // promise either way.
                sys::set_non_blocking(fd);
                Some(Handed { fd, kind, port })
            })
            .collect();
        Self { sockets }
    }

    #[cfg(not(unix))]
    pub fn classify(_fds: Vec<i32>) -> Self {
        Self::default()
    }
}

#[cfg(unix)]
mod sys {
    use super::{Kind, RawFd};
    use std::os::raw::c_void;

    /// Stream or datagram, straight from the kernel — the supervisor's
    /// configuration is not consulted, so a unit that says one thing and binds
    /// another cannot mislead us.
    pub(super) fn socket_kind(fd: RawFd) -> Option<Kind> {
        let mut kind: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `kind` and `len` are live locals of the right sizes, and the
        // call only reads from `fd`.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                &mut kind as *mut _ as *mut c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return None;
        }
        match kind {
            libc::SOCK_STREAM => Some(Kind::Stream),
            libc::SOCK_DGRAM => Some(Kind::Datagram),
            _ => None,
        }
    }

    /// The port the supervisor bound, read off the socket itself.
    pub(super) fn local_port(fd: RawFd) -> Option<u16> {
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        // SAFETY: `storage` is a live local large enough for any address the
        // kernel will write, and `len` says so.
        let rc = unsafe {
            libc::getsockname(fd, &mut storage as *mut _ as *mut libc::sockaddr, &mut len)
        };
        if rc != 0 {
            return None;
        }
        // The port sits at the same offset in both families, but read each
        // through its own type rather than relying on that.
        match storage.ss_family as libc::c_int {
            libc::AF_INET => {
                let addr = unsafe { &*(&storage as *const _ as *const libc::sockaddr_in) };
                Some(u16::from_be(addr.sin_port))
            }
            libc::AF_INET6 => {
                let addr = unsafe { &*(&storage as *const _ as *const libc::sockaddr_in6) };
                Some(u16::from_be(addr.sin6_port))
            }
            _ => None,
        }
    }

    /// Best-effort: a descriptor that cannot be marked is still usable, and
    /// refusing to serve over it would be the worse outcome.
    pub(super) fn set_close_on_exec(fd: RawFd) {
        // SAFETY: plain fcntl on a descriptor we own.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }

    pub(super) fn set_non_blocking(fd: RawFd) {
        // SAFETY: plain fcntl on a descriptor we own.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::os::raw::{c_char, c_int};
    use std::os::unix::io::RawFd;

    extern "C" {
        /// `launch_activate_socket(3)`. Answers only a process launchd started
        /// for this job; anything else gets `ESRCH` and no descriptors.
        fn launch_activate_socket(
            name: *const c_char,
            fds: *mut *mut c_int,
            count: *mut usize,
        ) -> c_int;
    }

    /// The name of the `Sockets` entry the installer writes into the plist.
    /// One entry carries every listener, TCP and UDP together, because
    /// `SO_TYPE` tells them apart on arrival.
    pub(super) const SOCKET_NAME: &str = "Listeners";

    pub(super) fn delivered_fds() -> Vec<RawFd> {
        let Ok(name) = std::ffi::CString::new(SOCKET_NAME) else {
            return Vec::new();
        };
        let mut fds: *mut c_int = std::ptr::null_mut();
        let mut count: usize = 0;
        // SAFETY: `name` outlives the call; `fds` and `count` are out
        // parameters the call fills in, and the array becomes ours to free.
        let rc = unsafe { launch_activate_socket(name.as_ptr(), &mut fds, &mut count) };
        if rc != 0 || fds.is_null() {
            return Vec::new();
        }
        // SAFETY: on success launchd guarantees `count` descriptors at `fds`.
        let out = unsafe { std::slice::from_raw_parts(fds, count) }.to_vec();
        // SAFETY: the array was allocated for us by launchd and is ours to
        // release; the descriptors it held stay open.
        unsafe { libc::free(fds as *mut libc::c_void) };
        out
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::io::RawFd;

    /// systemd's contract: delivered descriptors start at 3, immediately after
    /// the standard streams.
    const FIRST_FD: RawFd = 3;

    pub(super) fn delivered_fds() -> Vec<RawFd> {
        // `LISTEN_PID` guards against a child inheriting the variables and
        // claiming its parent's sockets.
        let for_us = std::env::var("LISTEN_PID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .map(|pid| pid == std::process::id())
            .unwrap_or(false);
        if !for_us {
            return Vec::new();
        }
        let count = std::env::var("LISTEN_FDS")
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);
        if count <= 0 {
            return Vec::new();
        }
        // Clear them so anything this daemon spawns does not believe it was
        // socket-activated too.
        std::env::remove_var("LISTEN_PID");
        std::env::remove_var("LISTEN_FDS");
        std::env::remove_var("LISTEN_FDNAMES");
        (FIRST_FD..FIRST_FD + count).collect()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;

    /// The classifier is the part that can be wrong in a way nothing else
    /// catches, so give it real sockets — bound by this process, since a test
    /// has no supervisor — and check it reads back what it was given.
    #[test]
    fn a_tcp_listener_and_a_udp_socket_are_told_apart_by_the_kernel() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let tcp_port = tcp.local_addr().unwrap().port();
        let udp_port = udp.local_addr().unwrap().port();

        let mut inherited = Inherited::classify(vec![tcp.as_raw_fd(), udp.as_raw_fd()]);
        assert_eq!(inherited.len(), 2);
        assert_eq!(
            inherited.describe(),
            format!("tcp/{tcp_port}, udp/{udp_port}")
        );

        // Asking for the wrong protocol on a port that exists must miss, or a
        // UDP resolver could be handed the TCP half of the same port.
        assert!(inherited.take_udp(tcp_port).is_none());
        assert!(inherited.take_tcp(udp_port).is_none());

        let claimed = inherited.take_tcp(tcp_port).expect("the tcp listener");
        assert_eq!(claimed.local_addr().unwrap().port(), tcp_port);
        // Claimed once and only once: a second caller must fall back to
        // binding rather than accept on a descriptor someone else owns.
        assert!(inherited.take_tcp(tcp_port).is_none());
        assert_eq!(inherited.len(), 1);

        let claimed_udp = inherited.take_udp(udp_port).expect("the udp socket");
        assert_eq!(claimed_udp.local_addr().unwrap().port(), udp_port);
        assert!(inherited.is_empty());

        // The originals were consumed by `from_raw_fd`; forget them so the
        // test does not close the same descriptors twice.
        std::mem::forget(tcp);
        std::mem::forget(udp);
    }

    /// Nothing delivered is the ordinary case, and it has to be quiet.
    #[test]
    fn an_empty_set_hands_out_nothing() {
        let mut none = Inherited::classify(Vec::new());
        assert!(none.is_empty());
        assert_eq!(none.describe(), "none");
        assert!(none.take_tcp(80).is_none());
        assert!(none.take_udp(53).is_none());
    }

    /// A descriptor that is not a socket, or is closed, must be dropped rather
    /// than panic or be handed out as a listener.
    #[test]
    fn a_descriptor_that_is_not_a_socket_is_ignored() {
        let file = std::fs::File::open("/dev/null").unwrap();
        let inherited = Inherited::classify(vec![file.as_raw_fd(), -1]);
        assert!(inherited.is_empty(), "{}", inherited.describe());
    }

    /// An inherited listener must not leak into php-fpm, node, or anything
    /// else the daemon spawns: a child holding port 80 open outlives the
    /// daemon and blocks the next one from binding.
    #[test]
    fn a_claimed_socket_does_not_survive_an_exec() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let fd = tcp.as_raw_fd();
        let _ = Inherited::classify(vec![fd]);
        // SAFETY: reading the flags of a descriptor this test owns.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert!(flags >= 0);
        assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    }
}
