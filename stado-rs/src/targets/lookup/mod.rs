//! Answering "which target is this" and "where does this service run": the
//! host directory, the naming rules, and the two lookup surfaces that read a
//! registry without changing it.

mod directory;
mod namespace;
mod registry_lookup;
mod service_lookup;

pub use directory::*;
pub use namespace::*;
pub use registry_lookup::identity_stem;
pub use service_lookup::*;
