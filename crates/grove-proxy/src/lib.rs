//! grove-proxy — HTTP/HTTPS reverse proxy + FastCGI client (PRD §6.2, §8.1).

pub mod fastcgi;
pub mod handler;
pub mod server;
pub mod state;
pub mod tls;

pub use fastcgi::FpmAddr;
pub use handler::FpmLocator;
pub use server::{serve_http, serve_https};
pub use state::SharedState;
pub use tls::SniResolver;
