use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not determine a config directory for the current platform")]
    NoConfigDir,

    #[error("config file at {path} is invalid: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("failed to serialize config: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    #[error("path {0} does not exist")]
    PathNotFound(PathBuf),

    #[error("site {0:?} is not registered")]
    UnknownSite(String),

    #[error("a site named {0:?} already exists")]
    DuplicateSite(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
