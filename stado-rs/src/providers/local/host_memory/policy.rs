//! Which declaration this host is measured against.
//!
//! One resolver, and it delegates the "which target am I" half to the disk
//! janitor's [`crate::providers::local::disk_cleanup::resolve_canonical_policy`].
//! That is deliberate: two passes on one host that disagreed about which
//! registry entry they were executing would be two hosts as far as every
//! report is concerned, and the identity rules — normalized hostname, the
//! declared `hostnames` list, exactly one match, `kind` must be `local` —
//! are the fleet's, not this module's.

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::schema::{self, MemoryReclaimPolicy};
use crate::providers::local::disk_cleanup::{
    canonical_json, resolve_canonical_policy, JanitorError,
};
use crate::targets::{ComputeTarget, RegistryStore};

/// The declaration in force on this host, and where it came from.
#[derive(Debug, Clone)]
pub struct ResolvedMemoryPolicy {
    pub target: ComputeTarget,
    pub policy: MemoryReclaimPolicy,
    /// SHA-256 of the canonical policy bytes, so a state file written under
    /// one declaration is never read as evidence about another.
    pub digest: String,
    /// True when the host declares none and the reporting default is in
    /// force.
    pub defaulted: bool,
}

/// Read the canonical registry through the configured backend.
async fn fetch_canonical_registry() -> Result<Value, JanitorError> {
    let store = RegistryStore::open()
        .await
        .map_err(|error| JanitorError::os(&error.to_string()))?;
    let text = store
        .read_versioned()
        .await
        .map_err(|error| JanitorError::os(&error.to_string()))?
        .ok_or_else(|| JanitorError::lookup("canonical registry object is absent"))?;
    serde_json::from_str(&text.content).map_err(|error| JanitorError::value(&error.to_string()))
}

/// Resolve the memory declaration for `hostname` from a registry document.
pub fn resolve_from_document(
    data: &Value,
    hostname: &str,
) -> Result<ResolvedMemoryPolicy, JanitorError> {
    let (target, _disk, _disk_digest, _disk_defaulted) = resolve_canonical_policy(data, hostname)?;
    let (policy, canonical, defaulted) = match schema::declared(&target) {
        Some(policy) => {
            let rendered = serde_json::to_value(&policy).map_err(|error| {
                JanitorError::value(&format!("declared memory policy is unreadable: {error}"))
            })?;
            (policy, canonical_json(&rendered), false)
        }
        None => {
            let policy = MemoryReclaimPolicy::reporting_default();
            let rendered = serde_json::to_value(&policy).map_err(|error| {
                JanitorError::value(&format!("reporting default is unreadable: {error}"))
            })?;
            (policy, canonical_json(&rendered), true)
        }
    };
    Ok(ResolvedMemoryPolicy {
        target,
        policy,
        digest: format!("{:x}", Sha256::digest(canonical.as_bytes())),
        defaulted,
    })
}

/// Resolve the memory declaration this host runs under, reading the
/// canonical registry.
pub async fn resolve_for_this_host(hostname: &str) -> Result<ResolvedMemoryPolicy, JanitorError> {
    let data = fetch_canonical_registry().await?;
    resolve_from_document(&data, hostname)
}
