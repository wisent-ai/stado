//! Dedicated verifier-grant constructors. Each verifier is an auth boundary:
//! it enforces its exact consumer name and a token file distinct from every
//! other grant, and it never routes through the credential store selector.

use super::client::Client;
use super::SkarbiecError;

impl Client {
    /// Dedicated verifier used only for namespace-scoped product object
    /// bearers. It never reuses the coordinator's broader Skarbiec grant.
    pub fn object_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::object_skarbiec_consumer() != crate::config::OBJECT_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "object verifier consumer must be {:?}",
                crate::config::OBJECT_API_VERIFIER_CONSUMER
            )));
        }
        if crate::config::object_skarbiec_token_file() == crate::config::skarbiec_token_file() {
            return Err(SkarbiecError::Deployment(
                "object verifier token file must be distinct from the coordinator grant"
                    .to_string(),
            ));
        }
        Self::direct(
            crate::config::object_skarbiec_url(),
            crate::config::object_skarbiec_consumer(),
            crate::config::object_skarbiec_token_file(),
        )
    }

    /// Dedicated verifier for immutable authenticated release publication.
    pub fn release_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::release_skarbiec_consumer()
            != crate::config::RELEASE_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "release verifier consumer must be {:?}",
                crate::config::RELEASE_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::release_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "release verifier token file must be distinct from coordinator and product-object verifier grants"
                    .to_string(),
            ));
        }
        Self::direct(
            crate::config::release_skarbiec_url(),
            crate::config::release_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for exact machine client bearers.
    pub fn machine_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::machine_skarbiec_consumer()
            != crate::config::MACHINE_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "machine verifier consumer must be {:?}",
                crate::config::MACHINE_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::machine_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::agent_skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
            || token_file == crate::config::release_skarbiec_token_file()
            || token_file == crate::config::service_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "machine verifier token file must be distinct from coordinator, workload-agent, object, release, and service verifier grants"
                    .to_string(),
            ));
        }
        Self::direct(
            crate::config::machine_skarbiec_url(),
            crate::config::machine_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for exact Stado push ingress client bearers.
    pub fn backend_push_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::backend_push_skarbiec_consumer()
            != crate::config::BACKEND_PUSH_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "backend push verifier consumer must be {:?}",
                crate::config::BACKEND_PUSH_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::backend_push_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::agent_skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
            || token_file == crate::config::release_skarbiec_token_file()
            || token_file == crate::config::machine_skarbiec_token_file()
            || token_file == crate::config::service_skarbiec_token_file()
            || token_file == crate::config::rate_limit_skarbiec_token_file()
            || token_file == crate::config::backend_messaging_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "backend push verifier token file must be distinct from every control, workload, messaging, and API verifier grant"
                    .to_string(),
            ));
        }
        Self::direct(
            crate::config::backend_push_skarbiec_url(),
            crate::config::backend_push_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for exact managed-service deployer bearers.
    pub fn service_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::service_skarbiec_consumer()
            != crate::config::SERVICE_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "service verifier consumer must be {:?}",
                crate::config::SERVICE_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::service_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
            || token_file == crate::config::release_skarbiec_token_file()
            || token_file == crate::config::machine_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "service verifier token file must be distinct from coordinator, product-object, and release verifier grants"
                    .to_string(),
            ));
        }
        Self::direct(
            crate::config::service_skarbiec_url(),
            crate::config::service_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for shared rate-limit client bearers.
    pub fn rate_limit_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::rate_limit_skarbiec_consumer()
            != crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "rate limit verifier consumer must be {:?}",
                crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::rate_limit_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
            || token_file == crate::config::release_skarbiec_token_file()
            || token_file == crate::config::machine_skarbiec_token_file()
            || token_file == crate::config::service_skarbiec_token_file()
            || token_file == crate::config::agent_skarbiec_token_file()
            || token_file == crate::config::backend_push_skarbiec_token_file()
            || token_file == crate::config::backend_messaging_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "rate limit verifier token file must be distinct from every other verifier grant"
                    .to_string(),
            ));
        }
        Self::direct(
            crate::config::rate_limit_skarbiec_url(),
            crate::config::rate_limit_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for finite integration client bearers.
    pub fn integration_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::integration_skarbiec_consumer()
            != crate::config::INTEGRATION_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "integration verifier consumer must be {:?}",
                crate::config::INTEGRATION_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::integration_skarbiec_token_file();
        if [
            crate::config::skarbiec_token_file(),
            crate::config::agent_skarbiec_token_file(),
            crate::config::object_skarbiec_token_file(),
            crate::config::release_skarbiec_token_file(),
            crate::config::machine_skarbiec_token_file(),
            crate::config::service_skarbiec_token_file(),
            crate::config::rate_limit_skarbiec_token_file(),
            crate::config::backend_push_skarbiec_token_file(),
            crate::config::backend_messaging_skarbiec_token_file(),
        ]
        .contains(&token_file)
        {
            return Err(SkarbiecError::Deployment(
                "integration verifier token file must be distinct from control-plane, workload-agent, messaging, and every other verifier grant"
                    .to_string(),
            ));
        }
        Self::direct(
            crate::config::integration_skarbiec_url(),
            crate::config::integration_skarbiec_consumer(),
            token_file,
        )
    }

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
            crate::config::backend_push_skarbiec_token_file(),
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
        )
    }
}
