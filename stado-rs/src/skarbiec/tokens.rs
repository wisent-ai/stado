//! Scoped bearer readers through dedicated verifier coordinates. Item reads
//! route through the globally selected credential store; when Skarbiec is the
//! backend, its scoped grants remain the authorization boundary.
//!
//! Every item here is the one a boundary declaration names in the Stado
//! configuration, so each is read as named ([`Client::read_declared_string`])
//! and never selected by role.

use super::{Client, SkarbiecError};

pub async fn read_integration_token(
    item: &str,
    field: &str,
) -> Result<Option<String>, SkarbiecError> {
    Client::stado()?.read_declared_string(item, field).await
}

/// Resolve one product object bearer through the dedicated verifier grant,
/// with the version the value was read under. Callers must select `item`
/// from the canonical namespace policy first.
pub async fn read_object_token_versioned(
    item: &str,
    field: &str,
) -> Result<Option<super::VersionedValue>, SkarbiecError> {
    Client::stado()?.read_declared_versioned(item, field).await
}

/// The version a read of the object bearer would answer now, without the
/// value; `None` when the store keeps no versions or the item is absent.
pub async fn read_object_token_revision(
    item: &str,
    field: &str,
) -> Result<Option<super::ItemVersion>, SkarbiecError> {
    Client::stado()?.read_declared_revision(item, field).await
}

/// Resolve one bearer field of the item that plays `role` through the
/// dashboard's verifier grant: a route whose bearer no declaration names
/// (host-health) selects its item by role, never by id.
pub async fn read_role_token(role: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::stado()?.read_string(role, field).await
}

pub async fn read_release_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::stado()?.read_declared_string(item, field).await
}

/// Read the release authority's private key through the one consumer the vault
/// authorizes for it. The field is fixed because the item carries exactly one.
pub async fn read_release_signing_key(item: &str) -> Result<Option<String>, SkarbiecError> {
    Client::stado()?
        .read_declared_string(item, "private_key")
        .await
}

pub async fn read_machine_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::stado()?.read_declared_string(item, field).await
}

/// Resolve one registry-API client bearer through the dedicated verifier
/// grant. The caller selects `item` from `registry_api.clients`, never from a
/// request.
pub async fn read_registry_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::stado()?.read_declared_string(item, field).await
}

pub async fn read_service_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::stado()?.read_declared_string(item, field).await
}
