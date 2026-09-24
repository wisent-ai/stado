//! Stado reads its vault through one client, configured by
//! `secrets.skarbiec`.

use super::super::client::Client;
use super::super::{GrantMode, SkarbiecError};

impl Client {
    /// Stado's own grant, `secrets.skarbiec`, read afresh on every request so a
    /// re-minted bearer takes effect without a restart.
    pub fn stado() -> Result<Self, SkarbiecError> {
        if crate::config::skarbiec_consumer() != "stado" {
            return Err(SkarbiecError::Deployment(format!(
                "secrets.skarbiec.consumer is {:?}; Stado's identity must be \"stado\"",
                crate::config::skarbiec_consumer()
            )));
        }
        Self::direct(
            crate::config::skarbiec_url(),
            crate::config::skarbiec_consumer(),
            crate::config::skarbiec_token_file(),
            GrantMode::RereadPerRequest,
        )
    }
}
