//! The whole-document checks: a registry read on its own, and a candidate
//! read against the document it would replace.

mod registry;
mod write;

pub use registry::*;
pub use write::*;
