//! The savings ledger: what a decision actually cost, and what it saved.
//!
//! [`outcomes`] writes the placement feedback and savings measurements a tick
//! is still missing, [`savings`] folds the savings records and their
//! measurements into a summary, and [`reports`] persists the cost documents
//! and reads the provider billing snapshot back.

pub(super) mod outcomes;
pub(super) mod reports;
pub(super) mod savings;

// `super::storage` for the moved calls that name `super::storage::<item>`
// verbatim in `outcomes`.
use crate::autonomy::storage;
