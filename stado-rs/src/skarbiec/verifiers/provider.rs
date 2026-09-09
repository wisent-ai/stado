//! The exact provider grant for one finite integration domain. The domain must
//! be configured, and its grant file must be isolated from the control-plane
//! grant and from every verifier grant.

use super::super::client::Client;
use super::super::{GrantMode, SkarbiecError};

impl Client {
    /// Exact provider grant for one finite integration domain.
    pub fn integration_provider(domain: &str) -> Result<Self, SkarbiecError> {
        let provider = crate::config::integration_provider(domain).ok_or_else(|| {
            SkarbiecError::Deployment(format!(
                "integration provider domain {domain:?} is not configured"
            ))
        })?;
        let token_file = provider.token_file();
        if [
            crate::config::skarbiec_token_file(),
            crate::config::agent_skarbiec_token_file(),
            crate::config::integration_skarbiec_token_file(),
            crate::config::object_skarbiec_token_file(),
            crate::config::release_skarbiec_token_file(),
            crate::config::machine_skarbiec_token_file(),
            crate::config::service_skarbiec_token_file(),
            crate::config::rate_limit_skarbiec_token_file(),
            crate::config::backend_messaging_skarbiec_token_file(),
        ]
        .contains(&token_file)
        {
            return Err(SkarbiecError::Deployment(format!(
                "integration provider token file for domain {domain:?} is not isolated"
            )));
        }
        Self::direct(
            crate::config::integration_provider_skarbiec_url(),
            provider.consumer(),
            token_file,
            GrantMode::RereadPerRequest,
        )
    }
}
