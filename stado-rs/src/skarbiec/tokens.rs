//! Scoped bearer readers through the dedicated verifier grants. These are
//! serve-side auth-boundary reads: they always talk to Skarbiec and never
//! route through the credential store selector.

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
