//! Scoped bearer readers through dedicated verifier coordinates. Item reads
//! route through the globally selected credential store; when Skarbiec is the
//! backend, its scoped grants remain the authorization boundary.

use super::{Client, SkarbiecError};

pub async fn read_integration_token(
    item: &str,
    field: &str,
) -> Result<Option<String>, SkarbiecError> {
    Client::integration_verifier()?
        .read_string(item, field)
        .await
}

/// Resolve one product object bearer through the dedicated verifier grant.
/// Callers must select `item` from the canonical namespace policy first.
pub async fn read_object_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::object_verifier()?.read_string(item, field).await
}

pub async fn read_release_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::release_verifier()?.read_string(item, field).await
}

/// Read the release authority's private key through the one consumer the vault
/// authorizes for it. The field is fixed because the item carries exactly one.
pub async fn read_release_signing_key(item: &str) -> Result<Option<String>, SkarbiecError> {
    Client::release_signing_reader()?
        .read_string(item, "private_key")
        .await
}

pub async fn read_machine_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::machine_verifier()?.read_string(item, field).await
}

pub async fn read_service_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::service_verifier()?.read_string(item, field).await
}
