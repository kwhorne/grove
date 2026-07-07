//! grove-core — site registry, driver detection, configuration and shared state.
//!
//! This crate is the deterministic, side-effect-free heart of Grove. It knows
//! *what* sites exist and *how* they should be served, but never binds ports or
//! touches the operating system. Higher-level crates (`grove-dns`,
//! `grove-proxy`, `grove-daemon`) consume the resolved state produced here.

pub mod config;
pub mod driver;
pub mod error;
pub mod paths;
pub mod registry;
pub mod reqlog;
pub mod site;

pub use config::Config;
pub use driver::Driver;
pub use error::{Error, Result};
pub use registry::SiteRegistry;
pub use reqlog::{RequestEntry, RequestLog};
pub use site::{ResolvedSite, SiteKind};
