//! The delivered identity of one product coordinate, read from the signed
//! release `stado release submit` publishes there, and the objects that
//! release still lacks.

mod objects;
mod pipeline;

pub(crate) use objects::missing_release_objects;
pub(crate) use pipeline::catalog_identity;
