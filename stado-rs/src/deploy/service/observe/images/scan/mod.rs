//! The unit-image pass: what the local launchd tree declares, the verdict for
//! one pair of images, the scan itself, and its public projections.

mod classify;
mod observe;
mod report;

pub use classify::*;
pub(crate) use observe::*;
pub use report::*;
