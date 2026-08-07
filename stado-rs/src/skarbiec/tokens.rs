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

pub async fn read_machine_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::machine_verifier()?.read_string(item, field).await
}

pub async fn read_service_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::service_verifier()?.read_string(item, field).await
}
