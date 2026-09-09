//! Reversible shutdown planning. This module never mutates infrastructure.
//!
//! `command` holds the entry point the subcommand dispatches to, `discover`
//! the ownership proof behind `--all-stado-owned`, `selector` the operator
//! selectors parsed into draft actions, and `finalize` the reconciliation of
//! one draft against the state the executors observed.

mod command;
mod discover;
mod finalize;
mod selector;

pub use command::run;
