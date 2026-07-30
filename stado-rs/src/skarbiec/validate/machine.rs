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
    let client = Client::machine_verifier()?;
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
            "machine verifier grant item set mismatch (missing=[{missing}], unexpected=[{unexpected}])"
        )));
    }

    let mut token_owners = HashMap::<Vec<u8>, String>::new();
    let object_client = Client::object_verifier()?;
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces while validating machine bearers: {}",
            problems.join("; ")
        ))
    })?;
    for (namespace, policy) in namespaces {
        if let Some(token) = object_client.read_string(policy.item(), "token").await? {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("object namespace {namespace}"),
            );
        }
    }
    let release_client = Client::release_verifier()?;
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid release_api.publishers while validating machine bearers: {}",
            problems.join("; ")
        ))
    })?;
    for (product, policy) in publishers {
        if let Some(token) = release_client.read_string(policy.item(), "token").await? {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("release publisher {product}"),
            );
        }
    }
    let service_client = Client::service_verifier()?;
    let deployers = crate::config::service_api_deployers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid service_api.deployers while validating machine bearers: {}",
            problems.join("; ")
        ))
    })?;
    for (product, policy) in deployers {
        if let Some(token) = service_client.read_string(policy.item(), "token").await? {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("service deployer {product}"),
            );
        }
    }
    for (name, policy) in clients {
        let token = client
            .read_string(policy.item(), "token")
            .await?
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
