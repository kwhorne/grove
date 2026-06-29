//! Driver system (PRD §6.3).
//!
//! A *driver* decides how a site's requests are served. Detection is based on
//! filesystem signatures (e.g. `artisan` + `public/index.php` ⇒ Laravel). The
//! built-in set is deliberately declarative so a driver can be selected without
//! requiring PHP to be installed — unlike Valet's `*ValetDriver.php`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    /// Laravel application — front controller in `public/index.php`, `artisan`.
    Laravel,
    /// WordPress install — `wp-config.php` / `wp-load.php`.
    WordPress,
    /// Generic PHP app with a front controller.
    Php,
    /// Static files served directly from the document root.
    Static,
    /// Reverse proxy to an already-running dev server (Vite, Node, …).
    Proxy,
}

impl Driver {
    pub fn as_str(self) -> &'static str {
        match self {
            Driver::Laravel => "laravel",
            Driver::WordPress => "wordpress",
            Driver::Php => "php",
            Driver::Static => "static",
            Driver::Proxy => "proxy",
        }
    }

    /// Whether this driver dispatches to a PHP-FPM pool.
    pub fn is_php(self) -> bool {
        matches!(self, Driver::Laravel | Driver::WordPress | Driver::Php)
    }
}

impl std::fmt::Display for Driver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where, relative to the site root, the web server should serve from, plus how
/// to dispatch a given request path.
#[derive(Debug, Clone)]
pub struct DriverPlan {
    pub driver: Driver,
    /// Absolute document root.
    pub document_root: PathBuf,
    /// Front controller relative to `document_root` for PHP apps.
    pub front_controller: Option<PathBuf>,
}

/// Auto-detect the most specific driver for a site root.
///
/// Order matters: most specific first. Returns `None` only if the path does not
/// exist; otherwise it always falls back to `Static`.
pub fn detect(root: &Path) -> Option<Driver> {
    if !root.exists() {
        return None;
    }
    if is_laravel(root) {
        return Some(Driver::Laravel);
    }
    if is_wordpress(root) {
        return Some(Driver::WordPress);
    }
    if is_php(root) {
        return Some(Driver::Php);
    }
    Some(Driver::Static)
}

/// Resolve a concrete serving plan for a detected/explicit driver.
pub fn plan(root: &Path, driver: Driver) -> DriverPlan {
    match driver {
        Driver::Laravel => DriverPlan {
            driver,
            document_root: root.join("public"),
            front_controller: Some(PathBuf::from("index.php")),
        },
        Driver::WordPress => DriverPlan {
            driver,
            document_root: root.to_path_buf(),
            front_controller: Some(PathBuf::from("index.php")),
        },
        Driver::Php => {
            // Prefer public/ if present, else serve from root.
            let docroot = if root.join("public/index.php").exists() {
                root.join("public")
            } else {
                root.to_path_buf()
            };
            DriverPlan {
                driver,
                document_root: docroot,
                front_controller: Some(PathBuf::from("index.php")),
            }
        }
        Driver::Static => DriverPlan {
            driver,
            document_root: if root.join("public").is_dir() {
                root.join("public")
            } else {
                root.to_path_buf()
            },
            front_controller: None,
        },
        Driver::Proxy => DriverPlan {
            driver,
            document_root: root.to_path_buf(),
            front_controller: None,
        },
    }
}

fn is_laravel(root: &Path) -> bool {
    root.join("artisan").is_file() && root.join("public/index.php").is_file()
}

fn is_wordpress(root: &Path) -> bool {
    root.join("wp-config.php").is_file()
        || root.join("wp-load.php").is_file()
        || root.join("wp-login.php").is_file()
}

fn is_php(root: &Path) -> bool {
    root.join("index.php").is_file()
        || root.join("public/index.php").is_file()
        || root.join("composer.json").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn detects_laravel() {
        let dir = std::env::temp_dir().join(format!("grove-laravel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        touch(&dir.join("artisan"));
        touch(&dir.join("public/index.php"));
        assert_eq!(detect(&dir), Some(Driver::Laravel));
        let plan = plan(&dir, Driver::Laravel);
        assert_eq!(plan.document_root, dir.join("public"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detects_static_fallback() {
        let dir = std::env::temp_dir().join(format!("grove-static-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        touch(&dir.join("index.html"));
        assert_eq!(detect(&dir), Some(Driver::Static));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_path_is_none() {
        assert_eq!(detect(Path::new("/nope/does/not/exist/grove")), None);
    }
}
