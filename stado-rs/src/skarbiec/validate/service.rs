//! Validate the complete managed-service authorization boundary: the
//! verifier sees exactly the mapped deployer items, each token is non-empty,
//! and no service bearer collides with another service, object, or release
//! bearer.

use std::collections::{BTreeSet, HashMap};

use sha2::{Digest, Sha256};

use super::super::{Client, SkarbiecError};

pub async fn validate_service_verifier() -> Result<usize, SkarbiecError> {
    let deployers = crate::config::service_api_deployers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid service_api.deployers: {}",
            problems.join("; ")
        ))
    })?;
    let client = Client::stado()?;
    let expected = deployers
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
    if !expected.is_subset(&visible) {
        let missing = expected
            .difference(&visible)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        return Err(SkarbiecError::Deployment(format!(
            "service verifier grant is missing items [{missing}]"
        )));
    }
    let mut token_owners = HashMap::<Vec<u8>, String>::new();
    let object_client = Client::stado()?;
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces while validating service bearers: {}",
            problems.join("; ")
        ))
    })?;
    let object_tokens = super::read_token_fields(
        &object_client,
        namespaces.values().map(|policy| policy.item()).collect(),
    )
    .await?;
    for ((namespace, _), token) in namespaces.iter().zip(object_tokens) {
        if let Some(token) = token {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("object namespace {namespace}"),
            );
        }
    }
    let release_client = Client::stado()?;
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid release_api.publishers while validating service bearers: {}",
            problems.join("; ")
        ))
    })?;
    let release_tokens = super::read_token_fields(
        &release_client,
        publishers.values().map(|policy| policy.item()).collect(),
    )
    .await?;
    for ((product, _), token) in publishers.iter().zip(release_tokens) {
        if let Some(token) = token {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("release publisher {product}"),
            );
        }
    }
    let deployer_tokens = super::read_token_fields(
        &client,
        deployers.values().map(|policy| policy.item()).collect(),
    )
    .await?;
    for ((product, policy), token) in deployers.iter().zip(deployer_tokens) {
        let token = token
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for service deployer {product}",
                    policy.item()
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, format!("service deployer {product}")) {
            return Err(SkarbiecError::Deployment(format!(
                "bearer values for {other} and service deployer {product} must be distinct"
            )));
        }
    }
    Ok(deployers.len())
}
