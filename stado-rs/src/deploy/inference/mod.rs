//! Narrow remote lifecycle for one digest-pinned vLLM container.

mod lifecycle;
pub mod process;
pub mod routes;
pub(super) mod support;

pub use lifecycle::{
    install, inventory, logs, probe, retire, status, update_reservation, verify_completion,
};
pub use support::startup_timeout;
