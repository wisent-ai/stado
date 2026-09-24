//! The client Stado reads the vault through: `Client::stado()`, its one
//! identity. It never routes through the credential store selector and it
//! declares `GrantMode::RereadPerRequest`, so a re-minted bearer is picked up
//! without a restart.
//!
//! API boundaries use the same client rather than separate grant files.

mod api;
