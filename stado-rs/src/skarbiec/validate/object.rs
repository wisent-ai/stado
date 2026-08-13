//! Startup/doctor validation for the complete object authorization boundary.
//! The verifier grant must expose exactly the mapped items, every token must
//! be present, and tokens must be pairwise distinct so no bearer can cross a
//! namespace even after an accidental duplicate secret rotation.

use std::collections::{BTreeSet, HashMap};

use sha2::{Digest, Sha256};

use super::super::{Client, SkarbiecError};

pub async fn validate_object_verifier() -> Result<usize, SkarbiecError> {
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces: {}",
            problems.join("; ")
        ))
    })?;
    let client = Client::object_verifier()?;
    let expected = namespaces
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
            "object verifier grant item set mismatch (missing=[{missing}], unexpected=[{unexpected}])"
        )));
    }

    let mut token_owners = HashMap::<Vec<u8>, &str>::new();
    // A Skarbiec request decrypts and rewrites shared vault/audit state.
    // Unbounded startup fan-out caused intermittent connection resets and
    // made the complete authorization boundary fail closed. Read serially:
    // startup latency is bounded by the outer timeout, while the vault sees
    // one operation at a time.
    for (namespace, policy) in namespaces {
        let token = client
            .read_string(policy.item(), "token")
            .await
            .map_err(|error| {
                SkarbiecError::Deployment(format!(
                    "reading {}/token for namespace {namespace} failed: {error}",
                    policy.item()
                ))
            })?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for namespace {namespace}",
                    policy.item()
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, namespace) {
            return Err(SkarbiecError::Deployment(format!(
                "object bearer values for namespaces {other} and {namespace} must be distinct"
            )));
        }
    }
    Ok(namespaces.len())
}
