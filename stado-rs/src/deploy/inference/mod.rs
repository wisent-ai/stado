//! Narrow remote lifecycle for one digest-pinned vLLM container.

mod install;
mod observe;
mod retire;
mod support;

pub use install::{install, update_reservation};
pub use observe::{inventory, logs, probe, status, verify_completion};
pub use retire::retire;
pub use support::startup_timeout;
