//! SSH host keys in the globally selected credential store:
//! `key add|ls|rm|install|check`, plus generation and rotation in [`rotate`].
//!
//! Private material is never printed. A remote call reads the target key from
//! the selected store, writes one owner-only transient file for `ssh -i`, then
//! removes it. There is no OpenSSH home-directory recourse: changing
//! `STADO_CREDENTIALS_STORE` is a credential migration, not a second lookup
//! path.

mod channel;
pub use channel::channel_argv;
mod commands;
pub mod rotate;
mod store;

pub use commands::{add, check, install, install_first_contact, ls, rm, AdoptOutcome};
pub use store::{authorized_keys_line, item_id};

pub(crate) use store::configured_client;

pub(in crate::cli::fleet::key) use store::{run_checked, settle_readable, ITEM_TYPE};
