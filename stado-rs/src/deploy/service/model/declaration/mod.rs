//! What the registry declares, checked against what the host can hold: the
//! launchd domain, the managed-product version, and the product environment.

mod domain;
mod environment;
mod unit_paths;
mod versions;

pub use domain::*;
pub use environment::*;
pub use unit_paths::*;
pub use versions::*;
