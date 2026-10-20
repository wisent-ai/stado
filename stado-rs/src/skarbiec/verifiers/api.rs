//! Stado reads its vault through one client, configured by
//! `secrets.skarbiec` and routed through the selected credential store.

use super::super::client::Client;
use super::super::{GrantMode, SkarbiecError};

impl Client {
    /// Stado's own grant, `secrets.skarbiec`, read afresh on every request so a
    /// re-minted bearer takes effect without a restart. Every read and write
    /// goes through the selected credential store: with Skarbiec selected the
    /// vault is asked with this grant exactly as before, and on a machine whose
    /// store is a file (`STADO_CREDENTIALS_STORE=file:///…`) the boundary
    /// verifiers, `stado service secret-sync`, the dashboard and the release
    /// catalogue answer from that file, so Stado works without Skarbiec.
    pub fn stado() -> Result<Self, SkarbiecError> {
        if crate::config::skarbiec_consumer() != "stado" {
            return Err(SkarbiecError::Deployment(format!(
                "secrets.skarbiec.consumer is {:?}; Stado's identity must be \"stado\"",
                crate::config::skarbiec_consumer()
            )));
        }
        Self::new(
            crate::config::skarbiec_url(),
            crate::config::skarbiec_consumer(),
            crate::config::skarbiec_token_file(),
            GrantMode::RereadPerRequest,
        )
    }
}
