//! Deciding what a host answered, and publishing that decision.
//!
//! `join` holds the truth table, `memory` the half of it a host publishes
//! about its own memory, `payload` the readers it takes the host's own
//! published words out of, and `report` the two documents the verdict leaves
//! this crate in.

mod join;
mod memory;
mod payload;
mod report;

pub use join::assemble;
pub use memory::MemoryGate;
pub use report::{gates_section, to_report};
