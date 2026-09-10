//! What a memory declaration IS: its shape, what makes one coherent, the
//! repairs a registry may name, and the named policies a host may be armed
//! with.
//!
//! Kept apart from [`super::execution`] because these four files answer
//! questions before anything runs — `stado registry validate` and every
//! programmatic writer reach them without a pass existing — while execution
//! reaches units, processes and recovery programs on a real host.

pub mod policies;
pub mod schema;
pub mod validate;
pub mod vocabulary;
