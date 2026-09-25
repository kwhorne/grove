//! Starting an idle on-demand database *before* the request that needs it.
//!
//! An on-demand MySQL takes about a third of a second to start, and without
//! this the first request after it went idle waits all of it: PHP boots,
//! connects, and only then does the server start. Two earlier signals are
//! available, and both come through Grove:
//!
//! - **the DNS lookup.** A browser resolves `myapp.test` before it connects,
//!   and often while the address is still being typed, which can be hundreds
//!   of milliseconds ahead;
//! - **the request itself**, which arrives before PHP has booted far enough to
//!   open a connection. On the sites this was measured against, a whole warm
//!   request took 45 ms, so this alone saves little; it is the fallback.
//!
//! Either one maps the site to the bundled services its `.env` uses and asks
//! each idle on-demand one to start, in the background. Nothing here waits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use grove_proxy::SharedState;
use grove_services::ServiceManager;

use crate::ondemand::Fronts;

/// Services a site's `.env` points at, cached until the file changes.
type Cached = (Option<SystemTime>, Vec<&'static str>);

pub struct Warmer {
    services: Arc<ServiceManager>,
    fronts: Arc<Fronts>,
    shared: SharedState,
    cache: std::sync::Mutex<HashMap<PathBuf, Cached>>,
}

const ON_DEMAND_CAPABLE: [&str; 4] = ["mysql", "postgres", "elyrasql", "redis"];

impl Warmer {
    pub fn new(services: Arc<ServiceManager>, fronts: Arc<Fronts>, shared: SharedState) -> Self {
        Self {
            services,
            fronts,
            shared,
            cache: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// A request for site `name` has arrived.
    pub fn site(&self, name: &str) {
        if !self.anything_on_demand() {
            return;
        }
        let path = match self.shared.registry.try_read() {
            Ok(r) => r.get(name).map(|s| s.path.clone()),
            Err(_) => return,
        };
        if let Some(path) = path {
            self.warm_for(&path);
        }
    }

    /// Someone looked up `host`.
    pub fn host(&self, host: &str) {
        if !self.anything_on_demand() {
            return;
        }
        let path = match self.shared.registry.try_read() {
            Ok(r) => r.by_hostname(host).map(|s| s.path.clone()),
            Err(_) => return,
        };
        if let Some(path) = path {
            self.warm_for(&path);
        }
    }

    fn anything_on_demand(&self) -> bool {
        ON_DEMAND_CAPABLE
            .iter()
            .any(|k| self.services.is_on_demand(k))
    }

    fn warm_for(&self, project: &Path) {
        if project.as_os_str().is_empty() {
            return;
        }
        for key in self.keys_for(project) {
            if self.services.is_on_demand(key) {
                self.fronts.warm(&self.services, key);
            }
        }
    }

    fn keys_for(&self, project: &Path) -> Vec<&'static str> {
        let env = project.join(".env");
        let mtime = std::fs::metadata(&env).and_then(|m| m.modified()).ok();
        if let Ok(cache) = self.cache.lock() {
            if let Some((seen, keys)) = cache.get(project) {
                if *seen == mtime {
                    return keys.clone();
                }
            }
        }
        let keys = std::fs::read_to_string(&env)
            .map(|text| services_in(&text, &|k| self.services.port_of(k)))
            .unwrap_or_default();
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(project.to_path_buf(), (mtime, keys.clone()));
        }
        keys
    }
}

/// Which of Grove's bundled services a `.env` connects to, going by where it
/// points: a loopback host and the port Grove runs that service on. A remote
/// database, or a port Grove does not own, is none of ours to start.
pub fn services_in(env: &str, port_of: &dyn Fn(&str) -> Option<u16>) -> Vec<&'static str> {
    let get = |key: &str| -> Option<String> {
        env.lines().find_map(|line| {
            let line = line.trim_start();
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (k, v) = line.split_once('=')?;
            (k.trim() == key).then(|| v.trim().trim_matches('"').trim_matches('\'').to_string())
        })
    };
    let local = |host: Option<String>| {
        matches!(
            host.as_deref().unwrap_or("127.0.0.1"),
            "127.0.0.1" | "localhost" | "::1" | ""
        )
    };
    let mut keys = Vec::new();

    let db_default = match get("DB_CONNECTION").as_deref() {
        Some("mysql") | Some("mariadb") => Some(3306),
        Some("pgsql") | Some("postgres") | Some("postgresql") => Some(5432),
        _ => None,
    };
    if let Some(default) = db_default {
        if local(get("DB_HOST")) {
            let port = get("DB_PORT")
                .and_then(|p| p.parse().ok())
                .unwrap_or(default);
            for key in ["mysql", "elyrasql", "postgres"] {
                if port_of(key) == Some(port) {
                    keys.push(key);
                }
            }
        }
    }

    let uses_redis = [
        "CACHE_STORE",
        "CACHE_DRIVER",
        "SESSION_DRIVER",
        "QUEUE_CONNECTION",
    ]
    .iter()
    .any(|k| get(k).as_deref() == Some("redis"));
    if uses_redis && local(get("REDIS_HOST")) {
        let port = get("REDIS_PORT")
            .and_then(|p| p.parse().ok())
            .unwrap_or(6379);
        if port_of("redis") == Some(port) {
            keys.push("redis");
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ports(k: &str) -> Option<u16> {
        match k {
            "mysql" => Some(3306),
            "elyrasql" => Some(3307),
            "postgres" => Some(5432),
            "redis" => Some(6379),
            _ => None,
        }
    }

    #[test]
    fn a_laravel_env_names_the_services_it_uses() {
        let env = "DB_CONNECTION=mysql\nDB_HOST=127.0.0.1\nDB_PORT=3306\n\
                   CACHE_STORE=redis\nREDIS_HOST=127.0.0.1\n";
        assert_eq!(services_in(env, &ports), vec!["mysql", "redis"]);
    }

    /// ElyraSQL speaks MySQL's protocol; the port is what tells them apart.
    #[test]
    fn the_port_decides_which_server_it_is() {
        assert_eq!(
            services_in("DB_CONNECTION=mysql\nDB_PORT=3307\n", &ports),
            vec!["elyrasql"]
        );
        assert_eq!(
            services_in("DB_CONNECTION=pgsql\n", &ports),
            vec!["postgres"]
        );
    }

    /// Not Grove's to start: a remote host, a port Grove does not run, SQLite,
    /// and Redis configured but not used.
    #[test]
    fn what_is_not_ours_is_left_alone() {
        assert!(services_in("DB_CONNECTION=mysql\nDB_HOST=10.0.0.5\n", &ports).is_empty());
        assert!(services_in("DB_CONNECTION=mysql\nDB_PORT=3308\n", &ports).is_empty());
        assert!(services_in("DB_CONNECTION=sqlite\n", &ports).is_empty());
        assert!(services_in("REDIS_HOST=127.0.0.1\nCACHE_STORE=database\n", &ports).is_empty());
    }
}
