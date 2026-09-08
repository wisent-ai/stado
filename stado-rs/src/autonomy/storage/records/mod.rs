//! One component per record kind this layer persists: `policy` is the
//! operator's declaration, `inventory` is the sealed snapshot of what exists,
//! `decisions` is what the planner chose, and `ledger` holds the records a
//! decision leaves behind.

pub(super) mod decisions;
pub(super) mod inventory;
pub(super) mod ledger;
pub(super) mod policy;
