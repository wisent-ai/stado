//! The client Stado reads the vault through: `Client::stado()`, its one
//! identity. It never routes through the credential store selector and it
//! declares `GrantMode::RereadPerRequest`, so a re-minted bearer is picked up
//! without a restart.
//!
//! API boundaries use the same client rather than separate grant files. What
//! a failed verifier answer says about the vault's keyring lock is read in
//! [`keyring_lock`].

mod api;
mod keyring_lock;

pub use keyring_lock::{keyring_lock_sentence, KEYRING_LOCK_PHRASES};
