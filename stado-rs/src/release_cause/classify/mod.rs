//! Reading a cause out of a recorded reason, deepest cause first.

mod decide;
mod envelope;
mod needles;
mod segments;

pub use decide::{classify, Classification};

pub(in crate::release_cause) use segments::bound;
