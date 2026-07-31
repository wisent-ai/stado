//! Validate the immutable release-publisher verifier and ensure its bearers
//! cannot collide with any product object bearer.

use std::collections::{BTreeSet, HashMap};

use sha2::{Digest, Sha256};

use super::super::{Client, SkarbiecError};

pub async fn validate_release_verifier() -> Result<usize, SkarbiecError> {
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid release_api.publishers: {}",
            problems.join("; ")
        ))
    })?;
    let client = Client::release_verifier()?;
    let expected = publishers
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = client
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        let missing = expected
            .difference(&visible)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let unexpected = visible
            .difference(&expected)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        return Err(SkarbiecError::Deployment(format!(
            "release verifier grant item set mismatch (missing=[{missing}], unexpected=[{unexpected}])"
        )));
    }

    let object_namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces: {}",
            problems.join("; ")
        ))
    })?;
    let object_client = Client::object_verifier()?;
    let mut token_owners = HashMap::<Vec<u8>, String>::new();
    // Both sweeps share their verifier client concurrently: the Skarbiec
    // listener is thread-per-connection, so serial reads would multiply the
    // vault's gpg latency by the item count for no benefit.
    let object_reads: Vec<(&str, Result<Option<String>, SkarbiecError>)> =
        futures::future::join_all(object_namespaces.iter().map(|(namespace, policy)| {
            let object_client = &object_client;
            async move {
                (
                    namespace.as_str(),
                    object_client.read_string(policy.item(), "token").await,
                )
            }
        }))
        .await;
    for (namespace, result) in object_reads {
        let token = result?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for namespace {namespace}",
                    object_namespaces[namespace].item()
                ))
            })?;
        token_owners.insert(
            Sha256::digest(token.as_bytes()).to_vec(),
            format!("object namespace {namespace}"),
        );
    }
    let publisher_reads: Vec<(&str, Result<Option<String>, SkarbiecError>)> =
        futures::future::join_all(publishers.iter().map(|(product, policy)| {
            let client = &client;
            async move {
                (
                    product.as_str(),
                    client.read_string(policy.item(), "token").await,
                )
            }
        }))
        .await;
    for (product, result) in publisher_reads {
        let token = result?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for release publisher {product}",
                    publishers[product].item()
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, format!("release publisher {product}")) {
            return Err(SkarbiecError::Deployment(format!(
                "bearer values for {other} and release publisher {product} must be distinct"
            )));
        }
    }
    Ok(publishers.len())
}
