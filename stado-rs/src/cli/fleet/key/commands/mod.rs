//! The commands over one target's stored pair: the operator-facing
//! `key add|ls|rm|install|check` in `manage`, and the first-contact install
//! that the `adopt` enrollment method rides in `adopt`.

mod adopt;
mod manage;

pub use adopt::{install_first_contact, AdoptOutcome};
pub use manage::{add, check, install, ls, rm};
