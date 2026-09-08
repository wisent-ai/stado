//! Deciding what a host answered, and publishing that decision.
//!
//! `join` holds the truth table, `payload` the readers it takes the host's
//! own published words out of, and `report` the two documents the verdict
//! leaves this crate in.

mod join;
mod payload;
mod report;

pub use join::assemble;
pub use report::{gates_section, to_report};
