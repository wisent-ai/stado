//! Declaration-driven placement for interactive and receipt-producing host work.
//!
//! Workload names, ownership, plan schemas, registry gates and report contracts
//! live in `stado-rs/data/workloads.json`. This module is the only runtime
//! reader. Adding a product workload is a declaration change, not another CLI
//! verb.

mod catalog;
mod commands;
mod plan;
mod runners;

pub use catalog::{WorkloadKind, DECLARATION_PATH};
pub use commands::{dispatch, WorkloadCommands};
