//! grove-services — local service supervisor.
//!
//! Ships two things so the user never installs anything separately:
//!   * a built-in **mail-catcher** (SMTP server capturing outgoing mail), and
//!   * a **bundled service manager** that downloads + supervises portable
//!     database/cache builds (PostgreSQL today) under `$GROVE_HOME/services`.

pub mod catalog;
pub mod convert;
pub mod mail;
pub mod manager;
pub mod store;

pub use catalog::{ServiceKind, ServiceSpec, CATALOG};
pub use convert::{convert as convert_database, DbConnSpec};
pub use mail::serve_smtp;
pub use manager::{ServiceManager, ServiceStatus};
pub use store::{CapturedEmail, EmailSummary, MailStore};
