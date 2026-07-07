//! `grove.toml` — a per-project, committed description of the environment a
//! project needs (PHP/Node versions, bundled services, HTTPS, dev processes).
//!
//! A teammate can then go from `git clone` to a running, identical local
//! environment with a single `grove up`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The parsed contents of a project's `grove.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    /// Site name (defaults to the directory name).
    #[serde(default)]
    pub name: Option<String>,
    /// PHP version to pin for this site (e.g. "8.4").
    #[serde(default)]
    pub php: Option<String>,
    /// Node.js major to pin for this site (e.g. "22").
    #[serde(default)]
    pub node: Option<String>,
    /// Serve over HTTPS (default true).
    #[serde(default = "default_secure")]
    pub secure: bool,
    /// Bundled services to ensure installed + running (`mysql`, `postgres`, `redis`).
    #[serde(default)]
    pub services: Vec<String>,
    /// Start dev processes (Vite + queue worker) as part of `grove up`.
    #[serde(default)]
    pub dev: bool,
}

fn default_secure() -> bool {
    true
}

impl Default for ProjectFile {
    fn default() -> Self {
        Self {
            name: None,
            php: None,
            node: None,
            secure: true,
            services: Vec::new(),
            dev: false,
        }
    }
}

impl ProjectFile {
    /// The conventional path of the project file inside `dir`.
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join("grove.toml")
    }

    /// Load `dir/grove.toml`, or `None` if it does not exist.
    pub fn load(dir: &Path) -> Result<Option<Self>, ProjectError> {
        let path = Self::path_in(dir);
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)?;
        let parsed = toml::from_str(&raw).map_err(|e| ProjectError::Parse(e.to_string()))?;
        Ok(Some(parsed))
    }

    /// The effective site name for a project directory.
    pub fn site_name(&self, dir: &Path) -> String {
        self.name.clone().unwrap_or_else(|| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("app")
                .to_string()
        })
    }

    /// Render a friendly, commented starter `grove.toml`.
    pub fn starter_template(name: &str, php: &str) -> String {
        format!(
            "# grove.toml — this project's local environment, committed to the repo.\n\
             # A teammate runs `grove up` after cloning to get an identical setup.\n\
             \n\
             name = \"{name}\"\n\
             php = \"{php}\"\n\
             # node = \"22\"\n\
             secure = true\n\
             \n\
             # Bundled services Grove should install + start for this project:\n\
             # services = [\"mysql\", \"redis\"]\n\
             services = []\n\
             \n\
             # Start the dev processes (Vite + queue worker) as part of `grove up`:\n\
             # dev = true\n"
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid grove.toml: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_defaults() {
        let dir = std::env::temp_dir().join(format!("grove-proj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            ProjectFile::path_in(&dir),
            "name = \"shop\"\nphp = \"8.3\"\nservices = [\"mysql\", \"redis\"]\ndev = true\n",
        )
        .unwrap();

        let pf = ProjectFile::load(&dir).unwrap().unwrap();
        assert_eq!(pf.site_name(&dir), "shop");
        assert_eq!(pf.php.as_deref(), Some("8.3"));
        assert!(pf.secure); // defaulted
        assert_eq!(pf.services, vec!["mysql", "redis"]);
        assert!(pf.dev);

        // No file → None.
        let empty = std::env::temp_dir().join(format!("grove-proj-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        assert!(ProjectFile::load(&empty).unwrap().is_none());

        // Name falls back to the directory basename.
        let pf2 = ProjectFile::default();
        assert_eq!(pf2.site_name(Path::new("/x/myapp")), "myapp");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
