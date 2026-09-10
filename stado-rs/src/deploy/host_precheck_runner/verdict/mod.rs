//! What the fleet is told: the scope a question is asked in, what one runner
//! or host reports, what a runner that will not start is saying, and the
//! shape an answer is read out of.

pub(super) mod diagnostics;
pub(super) mod report;
pub(super) mod scope;
pub(super) mod status;

pub use diagnostics::*;
pub use scope::*;
pub use status::*;
