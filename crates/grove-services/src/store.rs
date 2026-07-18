//! In-memory, capped store of captured emails.
//!
//! Mail is ephemeral test data, so a bounded ring buffer is the right model:
//! the newest N messages are kept and the oldest are dropped. Nothing is
//! persisted to disk, which also avoids leaking message contents.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

const MAX_MESSAGES: usize = 200;

/// A fully captured email plus its parsed parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedEmail {
    pub id: u64,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    /// RFC3339 timestamp of when Grove received it.
    pub received_at: String,
    /// Epoch milliseconds Grove received it, for correlating with the request
    /// timeline (see `MailStore::in_window`).
    #[serde(default)]
    pub received_ms: u128,
    pub size: usize,
    /// Raw DATA payload (headers + body).
    pub raw: String,
    /// Decoded text/plain body, if found.
    pub text: Option<String>,
    /// Decoded text/html body, if found.
    pub html: Option<String>,
}

/// Lightweight projection for list views.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailSummary {
    pub id: u64,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub received_at: String,
    #[serde(default)]
    pub received_ms: u128,
    pub size: usize,
}

impl From<&CapturedEmail> for EmailSummary {
    fn from(e: &CapturedEmail) -> Self {
        EmailSummary {
            id: e.id,
            from: e.from.clone(),
            to: e.to.clone(),
            subject: e.subject.clone(),
            received_at: e.received_at.clone(),
            received_ms: e.received_ms,
            size: e.size,
        }
    }
}

/// Thread-safe handle to the capped mail buffer.
#[derive(Clone, Default)]
pub struct MailStore {
    inner: Arc<Mutex<VecDeque<CapturedEmail>>>,
    next_id: Arc<AtomicU64>,
}

impl MailStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store a captured email, evicting the oldest if at capacity. Returns the
    /// assigned id.
    pub fn push(&self, mut email: CapturedEmail) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        email.id = id;
        let mut buf = self.inner.lock().unwrap();
        if buf.len() >= MAX_MESSAGES {
            buf.pop_front();
        }
        buf.push_back(email);
        id
    }

    /// Newest-first summaries.
    pub fn summaries(&self) -> Vec<EmailSummary> {
        let buf = self.inner.lock().unwrap();
        buf.iter().rev().map(EmailSummary::from).collect()
    }

    pub fn get(&self, id: u64) -> Option<CapturedEmail> {
        let buf = self.inner.lock().unwrap();
        buf.iter().find(|e| e.id == id).cloned()
    }

    pub fn clear(&self) -> usize {
        let mut buf = self.inner.lock().unwrap();
        let n = buf.len();
        buf.clear();
        n
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Summaries of emails captured within `[start_ms, end_ms]` (inclusive),
    /// newest first. Used to correlate mail with a request's time window.
    pub fn in_window(&self, start_ms: u128, end_ms: u128) -> Vec<EmailSummary> {
        let buf = self.inner.lock().unwrap();
        buf.iter()
            .rev()
            .filter(|e| e.received_ms >= start_ms && e.received_ms <= end_ms)
            .map(EmailSummary::from)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(subject: &str) -> CapturedEmail {
        CapturedEmail {
            id: 0,
            from: "a@b.test".into(),
            to: vec!["c@d.test".into()],
            subject: subject.into(),
            received_at: "now".into(),
            received_ms: 0,
            size: 10,
            raw: "raw".into(),
            text: None,
            html: None,
        }
    }

    #[test]
    fn assigns_incrementing_ids_newest_first() {
        let store = MailStore::new();
        let id1 = store.push(email("one"));
        let id2 = store.push(email("two"));
        assert_eq!((id1, id2), (1, 2));
        let s = store.summaries();
        assert_eq!(s[0].subject, "two"); // newest first
        assert_eq!(store.get(id1).unwrap().subject, "one");
    }

    #[test]
    fn clear_empties() {
        let store = MailStore::new();
        store.push(email("x"));
        assert_eq!(store.clear(), 1);
        assert!(store.is_empty());
    }

    #[test]
    fn in_window_filters_by_received_ms() {
        let store = MailStore::new();
        let at = |ms: u128, subj: &str| {
            let mut e = email(subj);
            e.received_ms = ms;
            store.push(e)
        };
        at(100, "early");
        at(150, "inside");
        at(500, "late");

        let hit = store.in_window(120, 200);
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].subject, "inside");

        // Inclusive bounds.
        assert_eq!(store.in_window(100, 100).len(), 1);
        assert_eq!(store.in_window(200, 400).len(), 0);
    }
}
