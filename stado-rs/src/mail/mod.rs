//! Deterministic billing/operations analysis of mail Skrzynka has received.
//!
//! Stado holds no mail credential and speaks to no mail provider: Skrzynka
//! owns the mailboxes, and `skrzynka` reads its store. Nothing here syncs,
//! labels, archives, or sends a message.
//!
//! The components: `error` holds the one error type, `skrzynka` the read of
//! Skrzynka's message list, `analysis` the published report shapes and their
//! aggregation, and `message` the deterministic per-message classifier.

mod analysis;
mod error;
mod message;
mod skrzynka;

pub use analysis::{summarize, MailAnalysis, MailAnalysisReport};
pub use error::MailError;
pub use message::analyze;
pub use skrzynka::{messages, SkrzynkaMessage};
