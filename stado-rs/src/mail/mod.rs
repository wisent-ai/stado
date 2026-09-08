//! Read-only Gmail search and deterministic billing/operations analysis.
//!
//! Authentication is resolved only from the `stado-gmail` Skarbiec item:
//! either a short-lived access token or centrally stored OAuth refresh
//! credentials. Stado never shells out to a cloud CLI and never modifies,
//! labels, archives, or sends messages.
//!
//! The components: `error` holds the one error type, `analysis` the
//! published report shapes and their aggregation, `client` the paging Gmail
//! REST calls, and `message` the deterministic per-message classifier.

mod analysis;
mod client;
mod error;
mod message;

pub use analysis::{summarize, MailAnalysis, MailAnalysisReport};
pub use client::GmailClient;
pub use error::MailError;
