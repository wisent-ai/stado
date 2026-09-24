//! Validate the complete machine-client authorization boundary. The verifier
//! sees exactly the mapped client items, every bearer is present, and machine
//! bearers are distinct from every other ingress bearer.

use std::collections::{BTreeSet, HashMap};

use sha2::{Digest, Sha256};

use super::super::{Client, SkarbiecError};

pub async fn validate_machine_verifier() -> Result<usize, SkarbiecError> {
    let clients = crate::config::machine_api_clients().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid machine_api.clients: {}",
            problems.join("; ")
        ))
    })?;
    let client = Client::stado()?;
    let expected = clients
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
            "machine verifier grant is missing items [{missing}]"
        )));
    }

    let mut token_owners = HashMap::<Vec<u8>, String>::new();
    let object_client = Client::stado()?;
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces while validating machine bearers: {}",
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
            "invalid release_api.publishers while validating machine bearers: {}",
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
    let service_client = Client::stado()?;
    let deployers = crate::config::service_api_deployers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid service_api.deployers while validating machine bearers: {}",
            problems.join("; ")
        ))
    })?;
    let service_tokens = super::read_token_fields(
        &service_client,
        deployers.values().map(|policy| policy.item()).collect(),
    )
    .await?;
    for ((product, _), token) in deployers.iter().zip(service_tokens) {
        if let Some(token) = token {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("service deployer {product}"),
            );
        }
    }
    let machine_tokens = super::read_token_fields(
        &client,
        clients.values().map(|policy| policy.item()).collect(),
    )
    .await?;
    for ((name, policy), token) in clients.iter().zip(machine_tokens) {
        let token = token
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for machine client {name}",
                    policy.item()
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, format!("machine client {name}")) {
            return Err(SkarbiecError::Deployment(format!(
                "bearer values for {other} and machine client {name} must be distinct"
            )));
        }
    }
    Ok(clients.len())
}
