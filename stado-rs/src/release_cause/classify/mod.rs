//! Reading a cause out of the structure that carries it, deepest cause first.

mod decide;
mod envelope;
mod segments;

pub use decide::{classify, classify_observed, Classification};

pub(in crate::release_cause) use segments::bound;
