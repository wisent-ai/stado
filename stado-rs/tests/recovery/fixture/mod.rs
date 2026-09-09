//! The two halves of this area's fixture.
//!
//! [`fleet`] is the isolated registry whose one target is the machine running
//! the test, so the product's current-host path executes here rather than
//! opening an ssh connection. [`unit`] is the launchd unit a case owns in this
//! login's own domain, with the guard that boots its label out again.

pub mod fleet;
pub mod unit;
