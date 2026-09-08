//! The fixed remote programs, grouped the way they are fed to a host: the
//! shared prelude, the lifecycle bodies, the two write bodies, and the
//! read-only queries.

mod deploy;
mod lifecycle;
mod prelude;
mod query;

pub(crate) use deploy::*;
pub use lifecycle::*;
pub use prelude::*;
pub(crate) use query::*;
