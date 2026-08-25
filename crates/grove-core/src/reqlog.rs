//! An in-memory ring buffer of recent HTTP requests Grove proxied.
//!
//! Grove sits in front of every `*.test` site, so it can record a lightweight,
//! framework-agnostic timeline of requests (method, path, status, duration)
//! with zero configuration and no per-app instrumentation. For each request it
//! also keeps the headers and (bounded) body so the request can be inspected
//! and **replayed** — a mini, framework-agnostic Telescope built into the proxy.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use time::macros::format_description;
use time::OffsetDateTime;

use crate::redact;

/// Largest request body we retain per entry (enough to replay typical form/JSON
/// posts without unbounded memory growth).
pub const MAX_BODY: usize = 1024 * 1024;

/// One proxied request, as surfaced to the CLI/GUI timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEntry {
    /// Stable id for detail lookup / replay.
    #[serde(default)]
    pub id: u64,
    /// Wall-clock time the request completed, UTC `HH:MM:SS.mmm` (the GUI
    /// reformats `epoch_ms` to local time).
    pub time: String,
    /// Epoch milliseconds, for stable sorting / relative display.
    pub epoch_ms: u128,
    /// Site name the request routed to.
    pub site: String,
    pub method: String,
    /// Path plus query string.
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    pub https: bool,
}

/// The captured request line + headers + body for a single entry (input to the
/// GUI detail view; body is lossy UTF-8, bounded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestDetail {
    pub id: u64,
    pub method: String,
    pub host: String,
    pub path: String,
    pub https: bool,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub body_truncated: bool,
}

/// Everything needed to re-issue a request (used in-process by the daemon; not
/// sent over IPC).
#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    pub host: String,
    pub path: String,
    pub https: bool,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What the caller records for one request.
pub struct Record<'a> {
    pub site: &'a str,
    pub host: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub status: u16,
    pub duration_ms: u64,
    pub https: bool,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Set when the caller already dropped part of the body.
    ///
    /// A streamed body is captured up to [`MAX_BODY`] as it passes, so what
    /// arrives here is *exactly* that long — which the length check in
    /// [`RequestLog::record`] cannot tell apart from a body that happened to fit.
    /// The caller has to say.
    pub body_truncated: bool,
}

struct Captured {
    entry: RequestEntry,
    host: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    body_truncated: bool,
}

/// A bounded, thread-safe log of the most recent requests.
pub struct RequestLog {
    inner: Mutex<VecDeque<Captured>>,
    cap: usize,
    next_id: AtomicU64,
}

impl RequestLog {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(cap.min(1024))),
            cap: cap.max(1),
            next_id: AtomicU64::new(1),
        }
    }

    /// Append a completed request, trimming the oldest beyond the capacity.
    /// Returns the assigned id.
    pub fn record(&self, rec: Record<'_>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now = OffsetDateTime::now_utc();
        let fmt = format_description!("[hour]:[minute]:[second].[subsecond digits:3]");
        let (body, truncated) = if rec.body.len() > MAX_BODY {
            (rec.body[..MAX_BODY].to_vec(), true)
        } else {
            (rec.body, rec.body_truncated)
        };
        let entry = RequestEntry {
            id,
            time: now.format(&fmt).unwrap_or_default(),
            epoch_ms: (now.unix_timestamp_nanos() / 1_000_000).max(0) as u128,
            site: rec.site.to_string(),
            method: rec.method.to_string(),
            path: rec.path.to_string(),
            status: rec.status,
            duration_ms: rec.duration_ms,
            https: rec.https,
        };
        let cap = Captured {
            entry,
            host: rec.host.to_string(),
            headers: rec.headers,
            body,
            body_truncated: truncated,
        };
        if let Ok(mut q) = self.inner.lock() {
            if q.len() >= self.cap {
                q.pop_front();
            }
            q.push_back(cap);
        }
        id
    }

    /// Drop all captured entries.
    pub fn clear(&self) {
        if let Ok(mut q) = self.inner.lock() {
            q.clear();
        }
    }

    /// The most recent requests (newest first), optionally filtered by site.
    pub fn snapshot(&self, site: Option<&str>, limit: usize) -> Vec<RequestEntry> {
        let q = match self.inner.lock() {
            Ok(q) => q,
            Err(_) => return Vec::new(),
        };
        q.iter()
            .rev()
            .filter(|c| site.map(|s| c.entry.site == s).unwrap_or(true))
            .take(limit)
            .map(|c| c.entry.clone())
            .collect()
    }

    /// The timeline entry (with timing) for one request, by id.
    pub fn entry(&self, id: u64) -> Option<RequestEntry> {
        let q = self.inner.lock().ok()?;
        q.iter().find(|c| c.entry.id == id).map(|c| c.entry.clone())
    }

    /// Headers + body for one request, for the detail view.
    /// One captured request, with credentials redacted.
    ///
    /// This is the shape that *leaves* the daemon — over IPC to the CLI and GUI,
    /// into the curl/`.http`/Pest snippets, into the explain bundle, and through
    /// the MCP tools to an AI assistant. Grove sits in front of every request, so
    /// without this it was handing out `Authorization` headers, session cookies
    /// and login passwords to all of them.
    ///
    /// [`Self::captured`] deliberately does not redact: it feeds `grove replay`,
    /// stays inside the daemon, and needs the real credentials to re-issue a
    /// request that works.
    pub fn detail(&self, id: u64) -> Option<RequestDetail> {
        let q = self.inner.lock().ok()?;
        let c = q.iter().find(|c| c.entry.id == id)?;
        let mut headers = c.headers.clone();
        redact::headers(&mut headers);
        Some(RequestDetail {
            id,
            method: c.entry.method.clone(),
            host: c.host.clone(),
            path: redact::path_with_query(&c.entry.path).into_owned(),
            https: c.entry.https,
            status: c.entry.status,
            headers,
            body: redact::body(&String::from_utf8_lossy(&c.body)).into_owned(),
            body_truncated: c.body_truncated,
        })
    }

    /// Everything needed to replay one request, credentials included.
    ///
    /// Not redacted, and not sent over IPC: a replay that dropped the session
    /// cookie would just produce a different request. See [`Self::detail`] for
    /// the shape that leaves the daemon.
    pub fn captured(&self, id: u64) -> Option<CapturedRequest> {
        let q = self.inner.lock().ok()?;
        let c = q.iter().find(|c| c.entry.id == id)?;
        Some(CapturedRequest {
            method: c.entry.method.clone(),
            host: c.host.clone(),
            path: c.entry.path.clone(),
            https: c.entry.https,
            headers: c.headers.clone(),
            body: c.body.clone(),
        })
    }
}

impl Default for RequestLog {
    fn default() -> Self {
        Self::new(500)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A login request, of the kind Grove sees constantly.
    fn login_record() -> Record<'static> {
        Record {
            site: "myapp.test",
            host: "myapp.test",
            method: "POST",
            path: "/login?api_key=sk-live-QUERY",
            status: 302,
            duration_ms: 12,
            https: true,
            headers: vec![
                ("Authorization".into(), "Bearer sk-live-HEADER".into()),
                ("Cookie".into(), "laravel_session=SESSIONVALUE".into()),
                ("Accept".into(), "text/html".into()),
            ],
            body: b"email=me%40example.com&password=hunter2".to_vec(),
            body_truncated: false,
        }
    }

    /// The seam this whole feature rests on: what leaves the daemon is redacted,
    /// what replays it is not.
    #[test]
    fn detail_is_redacted_and_replay_is_not() {
        let log = RequestLog::new(8);
        let id = log.record(login_record());

        let detail = log.detail(id).expect("detail");
        let rendered = format!("{detail:?}");
        for secret in ["sk-live-HEADER", "SESSIONVALUE", "hunter2", "sk-live-QUERY"] {
            assert!(
                !rendered.contains(secret),
                "{secret} leaked into RequestDetail: {rendered}"
            );
        }
        // Still legible: the names and the non-secret fields survive, or the
        // entry stops being worth keeping.
        assert!(detail.headers.iter().any(|(k, _)| k == "Authorization"));
        assert_eq!(
            detail
                .headers
                .iter()
                .find(|(k, _)| k == "Accept")
                .map(|(_, v)| v.as_str()),
            Some("text/html")
        );
        assert!(detail.body.contains("email=me%40example.com"));
        assert!(detail.path.starts_with("/login?api_key="));

        // Replay needs the real thing, and never crosses the IPC boundary.
        let captured = log.captured(id).expect("captured");
        let header_values = captured
            .headers
            .iter()
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        for secret in ["sk-live-HEADER", "SESSIONVALUE"] {
            assert!(
                header_values.contains(secret),
                "{secret} was stripped from the replay path, which would change the request"
            );
        }
        // The body is bytes here, not text.
        let raw_body = String::from_utf8_lossy(&captured.body);
        assert!(
            raw_body.contains("hunter2"),
            "the replayed body must be byte-for-byte what arrived: {raw_body}"
        );
        assert!(
            captured.path.contains("sk-live-QUERY"),
            "the replayed path must keep its query intact: {}",
            captured.path
        );
    }

    fn rec<'a>(site: &'a str, method: &'a str, path: &'a str, status: u16) -> Record<'a> {
        Record {
            site,
            host: "h.test",
            method,
            path,
            status,
            duration_ms: 1,
            https: true,
            headers: vec![],
            body: vec![],
            body_truncated: false,
        }
    }

    #[test]
    fn records_newest_first_filters_and_caps() {
        let log = RequestLog::new(3);
        log.record(rec("a", "GET", "/1", 200));
        log.record(rec("b", "POST", "/2", 404));
        log.record(rec("a", "GET", "/3", 500));
        let last = log.record(rec("a", "GET", "/4", 200)); // evicts /1 (cap 3)

        let all = log.snapshot(None, 10);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].path, "/4"); // newest first
        assert!(all.iter().all(|e| e.path != "/1")); // oldest evicted

        let only_a = log.snapshot(Some("a"), 10);
        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|e| e.site == "a"));

        assert_eq!(log.snapshot(None, 1).len(), 1);
        assert_eq!(log.captured(last).unwrap().path, "/4");
        assert!(log.detail(last).is_some());
        assert_eq!(log.entry(last).unwrap().path, "/4");
        assert!(log.entry(9999).is_none());
    }

    /// Regression: a body captured *as it streamed* stops at exactly `MAX_BODY`,
    /// so the length check alone cannot tell it apart from a body that fit. The
    /// caller's flag has to survive, or every export of a large upload silently
    /// claims to be complete.
    #[test]
    fn caller_reported_truncation_is_kept() {
        let log = RequestLog::new(2);
        let exactly_at_cap = vec![b'x'; MAX_BODY];
        let id = log.record(Record {
            site: "a",
            host: "h.test",
            method: "POST",
            path: "/",
            status: 200,
            duration_ms: 1,
            https: false,
            headers: vec![],
            body: exactly_at_cap,
            body_truncated: true,
        });
        let d = log.detail(id).unwrap();
        assert!(
            d.body_truncated,
            "a capture that stopped at the cap is still truncated"
        );
        assert_eq!(d.body.len(), MAX_BODY);
    }

    #[test]
    fn a_body_that_fits_is_not_truncated() {
        let log = RequestLog::new(2);
        let id = log.record(Record {
            site: "a",
            host: "h.test",
            method: "POST",
            path: "/",
            status: 200,
            duration_ms: 1,
            https: false,
            headers: vec![],
            body: b"small".to_vec(),
            body_truncated: false,
        });
        assert!(!log.detail(id).unwrap().body_truncated);
    }

    #[test]
    fn body_is_bounded() {
        let log = RequestLog::new(2);
        let big = vec![b'x'; MAX_BODY + 10];
        let id = log.record(Record {
            site: "a",
            host: "h.test",
            method: "POST",
            path: "/",
            status: 200,
            duration_ms: 1,
            https: false,
            headers: vec![],
            body: big,
            body_truncated: false,
        });
        let d = log.detail(id).unwrap();
        assert!(d.body_truncated);
        assert_eq!(d.body.len(), MAX_BODY);
    }
}
