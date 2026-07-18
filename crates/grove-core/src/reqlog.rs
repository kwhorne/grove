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
            (rec.body, false)
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
    pub fn detail(&self, id: u64) -> Option<RequestDetail> {
        let q = self.inner.lock().ok()?;
        let c = q.iter().find(|c| c.entry.id == id)?;
        Some(RequestDetail {
            id,
            method: c.entry.method.clone(),
            host: c.host.clone(),
            path: c.entry.path.clone(),
            https: c.entry.https,
            status: c.entry.status,
            headers: c.headers.clone(),
            body: String::from_utf8_lossy(&c.body).into_owned(),
            body_truncated: c.body_truncated,
        })
    }

    /// Everything needed to replay one request.
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
        });
        let d = log.detail(id).unwrap();
        assert!(d.body_truncated);
        assert_eq!(d.body.len(), MAX_BODY);
    }
}
