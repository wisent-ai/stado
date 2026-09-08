//! What the registry declares, checked against what the host can hold: the
//! launchd domain, the managed-product version, and the product environment.

mod domain;
mod environment;
mod versions;

pub use domain::*;
pub use environment::*;
pub use versions::*;
