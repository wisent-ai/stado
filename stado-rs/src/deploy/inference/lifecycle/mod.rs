//! The three states one digest-pinned container passes through on a host:
//! installed, observed while it serves, and retired.

mod install;
mod observe;
mod retire;

pub use install::{install, update_reservation};
pub use observe::{inventory, logs, probe, status, verify_completion};
pub use retire::retire;
