//! Whether a product's declared environment reaches the unit serving it: the
//! local unit read, the gap vocabulary, the finding, and the per-target scan.

mod gap;
mod local_unit;
mod scan;

// `report` holds `UnreachableProductEnvironment`'s inherent methods and the
// label match `scan` reaches as `super::report`; it re-exports nothing.
mod report;

pub use gap::*;
pub use local_unit::*;
pub use scan::*;
