//! grove-services — local service supervisor (PRD §6.5, §8.1).
//!
//! Ships two things so the user never installs anything separately:
//!   * a built-in **mail-catcher** (SMTP server capturing outgoing mail), and
//!   * a **bundled service manager** that downloads + supervises portable
//!     database/cache builds (PostgreSQL today) under `$GROVE_HOME/services`.

pub mod catalog;
pub mod mail;
pub mod manager;
pub mod store;

pub use catalog::{ServiceKind, ServiceSpec, CATALOG};
pub use mail::serve_smtp;
pub use manager::{ServiceManager, ServiceStatus};
pub use store::{CapturedEmail, EmailSummary, MailStore};
