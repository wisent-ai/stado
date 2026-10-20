//! What the grant passes read from the owner vault and the clock: one
//! consumer's recorded grant, whether the vault is a replica, and now. Each
//! read that cannot answer is the error naming why.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::cli::secrets::launcher_json;

/// Seconds since the Unix epoch on this host's clock.
pub(super) fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|error| format!("this host's clock is before the Unix epoch: {error}"))
}

/// The consumer's grant in the owner vault: when it ends, and the
/// `action:item#field` capabilities it carries. `None` when the vault holds
/// no grant for the consumer; a listing or row that does not say these is
/// the error naming what it lacks.
pub(super) fn grant_record(
    listing: &Value,
    consumer: &str,
) -> Result<Option<(u64, Vec<String>)>, String> {
    let grants = listing
        .as_array()
        .ok_or_else(|| format!("skarbiec grant list answered no array: {listing}"))?;
    let mut found: Option<(u64, Vec<String>)> = None;
    for grant in grants
        .iter()
        .filter(|grant| grant.get("consumer").and_then(Value::as_str) == Some(consumer))
    {
        let expires_at = grant
            .get("expires_at")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                format!("the owner vault lists {consumer} with no expires_at: {grant}")
            })?;
        let entries = grant
            .get("capabilities")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                format!("the owner vault lists {consumer} with no capabilities: {grant}")
            })?;
        let mut capabilities = Vec::new();
        for entry in entries {
            let (Some(action), Some(item)) = (
                entry.get("action").and_then(Value::as_str),
                entry.get("item").and_then(Value::as_str),
            ) else {
                return Err(format!(
                    "{consumer} holds a capability with no action or item: {entry}"
                ));
            };
            capabilities.push(match entry.get("field").and_then(Value::as_str) {
                Some(field) if !field.is_empty() => format!("{action}:{item}#{field}"),
                _ => format!("{action}:{item}"),
            });
        }
        if found
            .as_ref()
            .is_none_or(|(latest, _)| expires_at > *latest)
        {
            found = Some((expires_at, capabilities));
        }
    }
    Ok(found)
}

/// The bond this vault replicates, when `skarbiec bond status` says this
/// vault is a replica: the authority lives on that host, and a grant written
/// here is overwritten by the next pull. A status that cannot be read is the
/// error naming why, since the vault could be either.
pub(super) fn replica_of(launcher: &Path, vault: &Path) -> Result<Option<String>, String> {
    let status = launcher_json(launcher, vault, &["bond", "status"]).map_err(|error| {
        format!(
            "whether {} is a replica cannot be read: {error}",
            vault.display()
        )
    })?;
    let bonds = status
        .as_array()
        .ok_or_else(|| format!("skarbiec bond status answered no array: {status}"))?;
    Ok(bonds
        .iter()
        .find(|bond| bond.get("role").and_then(Value::as_str) == Some("replica"))
        .and_then(|bond| bond.get("bond").and_then(Value::as_str))
        .map(str::to_owned))
}
