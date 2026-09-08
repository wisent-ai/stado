//! Durable operation archive and compare-and-swap execution state.
//!
//! `records` holds the documents an operation is archived as, `archive` the
//! handle every phase drives and the read-only history subcommands over it,
//! `names` the ids and paths a document may be stored under, and `clock` the
//! stamps every record carries.

mod archive;
mod clock;
mod names;
mod records;

pub use archive::{dispatch, Journal};
pub use records::{ActionPhase, ActionState, OperationEvent, OperationState, Phase};
