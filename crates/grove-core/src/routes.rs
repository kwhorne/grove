//! How long each route usually takes, and which ones got slower.
//!
//! Grove times every request it hands to PHP or an upstream. That is enough to
//! notice the thing nobody notices until production: a route that took 40 ms
//! last week and takes 400 ms now, because a change added an N+1 query or
//! dropped an index. Requests are grouped by route — method plus path, with
//! ids and hashes folded to placeholders, so `/orders/17` and `/orders/18` are
//! one route — and each keeps the durations of its recent requests.
//!
//! A route's *typical* time is the median of its baseline window; its *recent*
//! time is the median of its last few requests. Medians, so the one slow
//! request after an idle database starts, or after OPcache recompiles a file,
//! moves nothing. A route is flagged slower when recent is at least twice
//! typical and at least 50 ms more. While it is flagged its baseline stops
//! taking new samples, so a regression cannot quietly become the new normal;
//! it clears when the route is fast again, or when it is reset to accept the
//! new speed.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Requests a route keeps for its typical time.
const BASELINE: usize = 50;
/// Requests a route needs before it has a typical time to compare against.
const MIN_BASELINE: usize = 10;
/// The requests "recent" is the median of.
const RECENT: usize = 5;
/// Routes kept across all sites; the least recently seen goes first.
const MAX_ROUTES: usize = 2000;

/// One route's numbers, as `grove routes` shows them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteSummary {
    pub site: String,
    /// `GET /orders/{id}`.
    pub route: String,
    /// Requests seen since the route was first seen or last reset.
    pub requests: u64,
    /// Median of the baseline window; `None` until there are enough.
    pub typical_ms: Option<u64>,
    /// Median of the last few requests.
    pub recent_ms: Option<u64>,
    /// Set while the route is flagged slower.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slower: Option<Slower>,
}

/// When and how a route got slower.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Slower {
    /// Unix time in ms of the request that tipped it.
    pub since_ms: u128,
    /// The typical time it was measured against.
    pub typical_ms: u64,
    /// The most recent slow request's id in the request log, for
    /// `grove explain` or `grove replay`.
    pub request_id: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Route {
    baseline: VecDeque<u32>,
    #[serde(skip)]
    recent: VecDeque<u32>,
    requests: u64,
    #[serde(skip)]
    seen: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slower: Option<Slower>,
}

/// A route that just changed state, for the daemon to report.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Slower { recent_ms: u64, typical_ms: u64 },
    Recovered { recent_ms: u64, typical_ms: u64 },
}

/// Per-route timing for every site, shared by the proxy and the daemon.
#[derive(Default)]
pub struct RouteStats {
    inner: Mutex<Inner>,
    dirty: AtomicBool,
}

#[derive(Default, Serialize, Deserialize)]
struct Inner {
    /// Keyed by `(site, route)` joined with a tab, which neither contains.
    routes: HashMap<String, Route>,
    #[serde(skip)]
    clock: u64,
}

fn key(site: &str, route: &str) -> String {
    format!("{site}\t{route}")
}

fn median(v: &VecDeque<u32>) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    let mut s: Vec<u32> = v.iter().copied().collect();
    s.sort_unstable();
    Some(s[s.len() / 2] as u64)
}

/// `recent` against `typical`: is it slower by enough to say so?
fn is_slower(recent: u64, typical: u64) -> bool {
    recent >= typical.saturating_mul(2) && recent >= typical + 50
}

/// Fast enough again to clear the flag: within half again of typical.
fn is_recovered(recent: u64, typical: u64) -> bool {
    recent <= typical + typical / 2 || recent < typical + 25
}

/// The route a path belongs to: the query dropped, and the segments that
/// are ids — numbers, UUIDs, long hex hashes — folded to placeholders.
pub fn route_of(method: &str, path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let mut out = String::with_capacity(path.len() + method.len() + 1);
    out.push_str(method);
    out.push(' ');
    let mut first = true;
    for seg in path.split('/') {
        if !first {
            out.push('/');
        }
        first = false;
        out.push_str(fold(seg));
    }
    if !out.ends_with('/') && path.is_empty() {
        out.push('/');
    }
    out
}

fn fold(seg: &str) -> &str {
    let hex = |c: char| c.is_ascii_hexdigit();
    if !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()) {
        "{id}"
    } else if seg.len() == 36
        && seg.chars().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                hex(c)
            }
        })
    {
        "{uuid}"
    } else if seg.len() == 26
        && seg
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        && seg.chars().any(|c| c.is_ascii_digit())
    {
        // ULID, as Laravel's HasUlids makes them.
        "{ulid}"
    } else if seg.len() >= 16 && seg.chars().all(hex) && seg.chars().any(|c| c.is_ascii_digit()) {
        "{hash}"
    } else {
        seg
    }
}

impl RouteStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Count one finished request. Returns a change of state, if this
    /// request made the route slower or brought it back.
    pub fn observe(
        &self,
        site: &str,
        route: &str,
        duration_ms: u64,
        request_id: u64,
        now_ms: u128,
    ) -> Option<Change> {
        let mut inner = self.inner.lock().ok()?;
        inner.clock += 1;
        let clock = inner.clock;
        let k = key(site, route);
        if !inner.routes.contains_key(&k) && inner.routes.len() >= MAX_ROUTES {
            if let Some(oldest) = inner
                .routes
                .iter()
                .min_by_key(|(_, r)| r.seen)
                .map(|(k, _)| k.clone())
            {
                inner.routes.remove(&oldest);
            }
        }
        let r = inner.routes.entry(k).or_default();
        r.seen = clock;
        r.requests += 1;
        let d = duration_ms.min(u32::MAX as u64) as u32;
        r.recent.push_back(d);
        if r.recent.len() > RECENT {
            let aged = r.recent.pop_front().expect("longer than RECENT");
            if r.slower.is_none() {
                r.baseline.push_back(aged);
                if r.baseline.len() > BASELINE {
                    r.baseline.pop_front();
                }
            }
        }
        self.dirty.store(true, Ordering::Relaxed);

        if r.baseline.len() < MIN_BASELINE || r.recent.len() < RECENT {
            return None;
        }
        let (typical, recent) = (median(&r.baseline)?, median(&r.recent)?);
        match &mut r.slower {
            None if is_slower(recent, typical) => {
                r.slower = Some(Slower {
                    since_ms: now_ms,
                    typical_ms: typical,
                    request_id,
                });
                Some(Change::Slower {
                    recent_ms: recent,
                    typical_ms: typical,
                })
            }
            Some(_) if is_recovered(recent, typical) => {
                r.slower = None;
                Some(Change::Recovered {
                    recent_ms: recent,
                    typical_ms: typical,
                })
            }
            Some(s) => {
                if d as u64 >= recent {
                    s.request_id = request_id;
                }
                None
            }
            None => None,
        }
    }

    /// Every route, optionally for one site, slowest-changed first.
    pub fn summaries(&self, site: Option<&str>) -> Vec<RouteSummary> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        let mut out: Vec<RouteSummary> = inner
            .routes
            .iter()
            .filter_map(|(k, r)| {
                let (s, route) = k.split_once('\t')?;
                if site.is_some_and(|want| want != s) {
                    return None;
                }
                Some(RouteSummary {
                    site: s.to_string(),
                    route: route.to_string(),
                    requests: r.requests,
                    typical_ms: (r.baseline.len() >= MIN_BASELINE)
                        .then(|| median(&r.baseline))
                        .flatten(),
                    recent_ms: median(&r.recent),
                    slower: r.slower.clone(),
                })
            })
            .collect();
        let ratio = |s: &RouteSummary| match (s.recent_ms, s.typical_ms) {
            (Some(r), Some(t)) => r as f64 / t.max(1) as f64,
            _ => 0.0,
        };
        out.sort_by(|a, b| {
            b.slower
                .is_some()
                .cmp(&a.slower.is_some())
                .then(ratio(b).total_cmp(&ratio(a)))
                .then(a.site.cmp(&b.site))
                .then(a.route.cmp(&b.route))
        });
        out
    }

    /// Forget routes: all of them, one site's, or one route of one site.
    /// Returns how many went.
    pub fn reset(&self, site: Option<&str>, route: Option<&str>) -> usize {
        let Ok(mut inner) = self.inner.lock() else {
            return 0;
        };
        let before = inner.routes.len();
        inner.routes.retain(|k, _| {
            let Some((s, r)) = k.split_once('\t') else {
                return false;
            };
            let hit = site.is_none_or(|want| want == s) && route.is_none_or(|want| want == r);
            !hit
        });
        self.dirty.store(true, Ordering::Relaxed);
        before - inner.routes.len()
    }

    /// Read what an earlier daemon saved. A missing or damaged file is a
    /// fresh start.
    pub fn load(path: &Path) -> Self {
        let inner: Inner = crate::securefs::read_json_or_quarantine(path);
        Self {
            inner: Mutex::new(inner),
            dirty: AtomicBool::new(false),
        }
    }

    /// Write the baselines out if anything changed since the last save.
    pub fn save_if_changed(&self, path: &Path) -> std::io::Result<()> {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return Ok(());
        }
        let json = match self.inner.lock() {
            Ok(inner) => serde_json::to_string(&*inner).unwrap_or_default(),
            Err(_) => return Ok(()),
        };
        if let Err(e) = crate::securefs::write_private_atomic(path, json) {
            self.dirty.store(true, Ordering::Relaxed);
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_fold_and_words_do_not() {
        assert_eq!(route_of("GET", "/orders/17?page=2"), "GET /orders/{id}");
        assert_eq!(
            route_of("GET", "/u/9b2e6f1c-3d4a-4b5c-8d9e-0f1a2b3c4d5e/edit"),
            "GET /u/{uuid}/edit"
        );
        assert_eq!(
            route_of("GET", "/p/01J8Z3K4M5N6P7Q8R9S0T1V2W3"),
            "GET /p/{ulid}"
        );
        assert_eq!(
            route_of("GET", "/build/assets/app-4f3a9c0e1b2d3e4f.js"),
            "GET /build/assets/app-4f3a9c0e1b2d3e4f.js"
        );
        assert_eq!(
            route_of("GET", "/verify/4f3a9c0e1b2d3e4f5a6b"),
            "GET /verify/{hash}"
        );
        assert_eq!(
            route_of("POST", "/livewire/update"),
            "POST /livewire/update"
        );
        assert_eq!(route_of("GET", "/"), "GET /");
        assert_eq!(route_of("GET", "/?q=1"), "GET /");
        // A word made of hex letters is still a word.
        assert_eq!(
            route_of("GET", "/deadbeefcafebabe"),
            "GET /deadbeefcafebabe"
        );
    }

    fn feed(stats: &RouteStats, ms: u64, n: usize, first_id: u64) -> Vec<Change> {
        (0..n)
            .filter_map(|i| stats.observe("shop", "GET /orders", ms, first_id + i as u64, 0))
            .collect()
    }

    fn only(stats: &RouteStats) -> RouteSummary {
        stats.summaries(None).remove(0)
    }

    /// Steady and then five times slower: flagged once, pointing at a slow
    /// request, and the baseline does not drift up while it stays slow.
    #[test]
    fn a_route_that_gets_slower_is_flagged_and_stays_flagged() {
        let stats = RouteStats::new();
        assert!(feed(&stats, 40, 20, 1).is_empty());
        assert_eq!(only(&stats).typical_ms, Some(40));

        let changes = feed(&stats, 200, 3, 100);
        assert_eq!(
            changes,
            vec![Change::Slower {
                recent_ms: 200,
                typical_ms: 40
            }]
        );
        // A long run of slow requests later, it still measures against 40.
        assert!(feed(&stats, 200, 100, 200).is_empty());
        let s = only(&stats);
        assert_eq!(s.typical_ms, Some(40));
        assert_eq!(s.recent_ms, Some(200));
        assert_eq!(s.slower.as_ref().unwrap().typical_ms, 40);
        assert_eq!(s.slower.unwrap().request_id, 299);

        // Fixed: it clears.
        let back = feed(&stats, 42, 3, 400);
        assert_eq!(
            back,
            vec![Change::Recovered {
                recent_ms: 42,
                typical_ms: 40
            }]
        );
        assert!(only(&stats).slower.is_none());
    }

    /// One or two slow requests — a cold start, a recompile — are not a
    /// regression.
    #[test]
    fn a_spike_is_not_a_regression() {
        let stats = RouteStats::new();
        feed(&stats, 40, 20, 1);
        assert!(feed(&stats, 900, 2, 50).is_empty());
        assert!(feed(&stats, 40, 5, 60).is_empty());
        assert!(only(&stats).slower.is_none());
    }

    /// Small numbers double easily: 3 ms to 8 ms is not worth a word.
    #[test]
    fn fast_routes_need_a_real_difference() {
        let stats = RouteStats::new();
        feed(&stats, 3, 20, 1);
        assert!(feed(&stats, 30, 10, 50).is_empty());
        assert!(!feed(&stats, 60, 5, 70).is_empty());
    }

    /// No verdict before there is a baseline to hold it against.
    #[test]
    fn nothing_is_judged_on_too_few_requests() {
        let stats = RouteStats::new();
        feed(&stats, 40, 6, 1);
        assert!(feed(&stats, 400, 5, 10).is_empty());
        assert_eq!(only(&stats).typical_ms, None);
    }

    #[test]
    fn reset_accepts_the_new_speed_and_saving_round_trips() {
        let dir = std::env::temp_dir().join(format!("grove-routes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("routes.json");

        let stats = RouteStats::new();
        feed(&stats, 40, 20, 1);
        stats.observe("blog", "GET /", 10, 99, 0);
        stats.save_if_changed(&file).unwrap();
        let again = RouteStats::load(&file);
        let shop = again.summaries(Some("shop")).remove(0);
        // Baselines survive; the in-flight recent window does not.
        assert_eq!(shop.typical_ms, Some(40));
        assert_eq!(shop.requests, 20);

        assert_eq!(again.reset(Some("shop"), Some("GET /orders")), 1);
        assert_eq!(again.summaries(None).len(), 1);
        assert_eq!(again.reset(None, None), 1);
        assert!(again.summaries(None).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
