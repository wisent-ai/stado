//! Declaration-driven placement for interactive and receipt-producing host work.
//!
//! Workload names, ownership, plan schemas, registry gates and report contracts
//! live in `stado-rs/data/work/workloads.json`. This module is the only runtime
//! reader. Adding a product workload is a declaration change, not another CLI
//! verb.

mod catalog;
mod commands;
mod plan;
mod runners;
mod unmet;

pub use catalog::workload as declared;
pub use catalog::{WorkloadKind, WorkloadReservation, DECLARATION_PATH};
pub use commands::{dispatch, WorkloadCommands};
/// The batches queued harness sessions carry, from the catalog: the janitor
/// reads a host's sessions by them to prove which checkouts they cloned.
pub(crate) use runners::session_batches;
