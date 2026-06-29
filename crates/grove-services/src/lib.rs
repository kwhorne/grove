//! grove-services — local service supervisor (PRD §6.5, §8.1).
//!
//! v1 ships a built-in mail-catcher: an SMTP server that captures outgoing mail
//! locally so developers can inspect what their app sends without a real mail
//! provider. DB/Redis supervision is planned for a later iteration.

pub mod mail;
pub mod store;

pub use mail::serve_smtp;
pub use store::{CapturedEmail, EmailSummary, MailStore};
