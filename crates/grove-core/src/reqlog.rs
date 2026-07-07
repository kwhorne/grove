//! An in-memory ring buffer of recent HTTP requests Grove proxied.
//!
//! Grove sits in front of every `*.test` site, so it can record a lightweight,
//! framework-agnostic timeline of requests (method, path, status, duration)
//! with zero configuration and no per-app instrumentation.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use time::macros::format_description;
use time::OffsetDateTime;

/// One proxied request, as surfaced to the CLI/GUI timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEntry {
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

/// A bounded, thread-safe log of the most recent requests.
pub struct RequestLog {
    inner: Mutex<VecDeque<RequestEntry>>,
    cap: usize,
}

impl RequestLog {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(cap.min(1024))),
            cap: cap.max(1),
        }
    }

    /// Append a completed request, trimming the oldest beyond the capacity.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        site: &str,
        method: &str,
        path: &str,
        status: u16,
        duration_ms: u64,
        https: bool,
    ) {
        let now = OffsetDateTime::now_utc();
        let fmt = format_description!("[hour]:[minute]:[second].[subsecond digits:3]");
        let entry = RequestEntry {
            time: now.format(&fmt).unwrap_or_default(),
            epoch_ms: (now.unix_timestamp_nanos() / 1_000_000).max(0) as u128,
            site: site.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            status,
            duration_ms,
            https,
        };
        if let Ok(mut q) = self.inner.lock() {
            if q.len() >= self.cap {
                q.pop_front();
            }
            q.push_back(entry);
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
            .filter(|e| site.map(|s| e.site == s).unwrap_or(true))
            .take(limit)
            .cloned()
            .collect()
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

    #[test]
    fn records_newest_first_filters_and_caps() {
        let log = RequestLog::new(3);
        log.record("a", "GET", "/1", 200, 5, true);
        log.record("b", "POST", "/2", 404, 9, false);
        log.record("a", "GET", "/3", 500, 1, true);
        log.record("a", "GET", "/4", 200, 2, true); // evicts /1 (cap 3)

        let all = log.snapshot(None, 10);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].path, "/4"); // newest first
        assert!(all.iter().all(|e| e.path != "/1")); // oldest evicted

        let only_a = log.snapshot(Some("a"), 10);
        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|e| e.site == "a"));

        assert_eq!(log.snapshot(None, 1).len(), 1);
    }
}
