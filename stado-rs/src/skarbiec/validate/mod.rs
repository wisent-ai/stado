//! Startup/doctor validation of the verifier authorization boundaries. Each
//! validator asserts its grant sees exactly the mapped items and that bearer
//! values are pairwise distinct, so no client token can cross a boundary.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use super::{read_grant, Client, SkarbiecError};

mod machine;
mod object;
mod release;
mod service;

pub use machine::validate_machine_verifier;
pub use object::validate_object_verifier;
pub use release::validate_release_verifier;
pub use service::validate_service_verifier;

/// Read one `token` field per item through one shared verifier client, with
/// bounded concurrency. The bound stays below Skarbiec's own request and GPG
/// admission limits, so a verifier sweep makes progress without monopolizing
/// credential delivery. Results come back in the same order as `items`.
pub(crate) async fn read_token_fields(
    client: &Client,
    items: Vec<&str>,
) -> Result<Vec<Option<String>>, SkarbiecError> {
    use futures::stream::StreamExt;

    let in_flight = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or_else(|_| std::num::NonZeroUsize::MIN.get());
    futures::stream::iter(
        items
            .into_iter()
            .map(|item| client.read_string(item, "token")),
    )
    .buffered(in_flight)
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .collect()
}

/// Validate the auth verifier independently from all provider domains.
pub async fn validate_integration_verifier() -> Result<usize, SkarbiecError> {
    let clients = crate::config::integration_clients().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid integration.clients: {}",
            problems.join("; ")
        ))
    })?;
    let verifier = Client::integration_verifier()?;
    let expected = clients
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = verifier
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        return Err(SkarbiecError::Deployment(
            "integration verifier grant item set mismatch".to_string(),
        ));
    }

    let verifier_grant = read_grant(crate::config::integration_skarbiec_token_file())?;
    let verifier_digest = Sha256::digest(verifier_grant.as_bytes()).to_vec();
    let mut bearer_digests = BTreeSet::new();
    for (name, policy) in clients {
        let bearer = verifier
            .read_string(policy.item(), "token")
            .await?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "integration bearer item is missing for client {name:?}"
                ))
            })?;
        let digest = Sha256::digest(bearer.as_bytes()).to_vec();
        if digest == verifier_digest || !bearer_digests.insert(digest) {
            return Err(SkarbiecError::Deployment(
                "integration verifier and all client bearer values must be distinct".to_string(),
            ));
        }
    }
    Ok(clients.len())
}

pub async fn validate_integration_provider(domain: &str) -> Result<usize, SkarbiecError> {
    let policy = crate::config::integration_provider(domain).ok_or_else(|| {
        SkarbiecError::Deployment(format!(
            "integration provider domain {domain:?} is not configured"
        ))
    })?;
    let provider = Client::integration_provider(domain)?;
    let expected = policy.items().iter().cloned().collect::<BTreeSet<_>>();
    let visible = provider
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        return Err(SkarbiecError::Deployment(format!(
            "integration provider grant item set mismatch for domain {domain:?}"
        )));
    }
    Ok(expected.len())
}

pub async fn validate_integration_boundary() -> Result<usize, SkarbiecError> {
    let mut total = validate_integration_verifier().await?;
    let providers = crate::config::integration_providers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid integration.providers: {}",
            problems.join("; ")
        ))
    })?;
    for domain in providers.keys() {
        total += validate_integration_provider(domain).await?;
    }
    Ok(total)
}
