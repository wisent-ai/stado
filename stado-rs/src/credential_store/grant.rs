//! Adding field reads to an existing Skarbiec consumer without rotating it.
//!
//! Skarbiec grants are per item, so an item written one second ago is readable
//! by nobody: the consumer that every credential delivery authenticates as does
//! not gain a capability by the write. A minted key is therefore dead until
//! someone widens that consumer's grant, and until this module existed the only
//! thing that could do it was an operator running a script by hand.
//!
//! `skarbiec token-mint` writes a whole grant, not a delta: it replaces the
//! stored capability list and, unless it is handed the current bearer, mints a
//! new one and keeps only the new hash. For a consumer like `local-operator`,
//! which already carries a four-figure capability list, minting by hand is how a
//! fleet loses its credentials — one forgotten capability or one rotated bearer
//! and every delivery starts failing.
//!
//! So this reads the live grant, refuses unless the consumer's owner-only token
//! file still hashes to the bearer the vault recorded (a bearer that cannot be
//! reproduced must not be replaced), takes the union of the existing
//! capabilities with the ones requested, preserves the remaining TTL, and
//! re-mints with `--token-file` so the bearer is written back unchanged. The
//! vault is copied first and the grant is measured before and after. Running it
//! twice changes nothing.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::owner;
use crate::skarbiec::SkarbiecError;

/// The only action this grants. Widening a consumer's writes is a deliberate
/// act with a different blast radius and does not belong on a mint path.
const ACTION: &str = "read";

/// What one call settled, for callers that report it.
#[derive(Clone, Debug)]
pub struct GrantOutcome {
    /// Capabilities the consumer held before the mint.
    pub held_before: usize,
    /// Capabilities the consumer holds now.
    pub held_after: usize,
    /// Capabilities this call added; empty means the grant already covered the
    /// request and nothing was written.
    pub added: Vec<String>,
    /// Seconds left on the preserved TTL.
    pub expires_in: i64,
    /// Vault copy taken before the mint, absent when nothing was written.
    pub backup: Option<PathBuf>,
}

impl GrantOutcome {
    /// Whether this call changed the vault.
    pub fn wrote(&self) -> bool {
        !self.added.is_empty()
    }
}

fn deployment(message: String) -> SkarbiecError {
    SkarbiecError::Deployment(message)
}

/// `action:item#field`, the spelling `token-mint --capabilities` takes and the
/// vault records.
fn encode(capability: &Value) -> String {
    let action = capability
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let item = capability
        .get("item")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match capability.get("field").and_then(Value::as_str) {
        Some(field) => format!("{action}:{item}#{field}"),
        None => format!("{action}:{item}"),
    }
}

fn now_seconds() -> i64 {
    chrono::Utc::now().timestamp()
}

fn read_vault(vault: &Path) -> Result<Value, SkarbiecError> {
    let body = std::fs::read_to_string(vault)
        .map_err(|error| deployment(format!("cannot read vault {}: {error}", vault.display())))?;
    serde_json::from_str(&body).map_err(|error| {
        deployment(format!(
            "vault {} is not valid JSON: {error}",
            vault.display()
        ))
    })
}

fn grant_of<'a>(document: &'a Value, consumer: &str) -> Result<&'a Value, SkarbiecError> {
    document
        .get("tokens")
        .and_then(|tokens| tokens.get(consumer))
        .ok_or_else(|| {
            deployment(format!(
                "no grant for consumer {consumer} in the owner vault; mint it deliberately first"
            ))
        })
}

/// Grant `consumer` a read on each of `fields` of `item`, keeping its bearer,
/// its remaining TTL, and every capability it already holds.
///
/// `token_file` is the consumer's own owner-only bearer file. It is required
/// rather than derived: the bearer is what makes this a widening instead of a
/// rotation, and a caller that cannot name it must not be minting for this
/// consumer at all.
pub fn grant_field_reads(
    consumer: &str,
    token_file: &Path,
    item: &str,
    fields: &[&str],
) -> Result<GrantOutcome, SkarbiecError> {
    let binary = owner::binary()?;
    let vault = owner::vault()?;
    let document = read_vault(&vault)?;
    let grant = grant_of(&document, consumer)?;
    let existing = grant
        .get("capabilities")
        .and_then(Value::as_array)
        .ok_or_else(|| deployment(format!("the {consumer} grant carries no capability list")))?
        .clone();
    let expires_at = grant
        .get("expires_at")
        .and_then(Value::as_i64)
        .ok_or_else(|| deployment(format!("the {consumer} grant carries no expiry")))?;
    let recorded_hash = grant
        .get("hash")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let audience = grant
        .get("audience")
        .and_then(Value::as_str)
        .unwrap_or(consumer)
        .to_string();
    let remaining = expires_at - now_seconds();

    let held: Vec<String> = existing.iter().map(encode).collect();
    let wanted: Vec<String> = fields
        .iter()
        .map(|field| format!("{ACTION}:{item}#{field}"))
        .collect();
    let added: Vec<String> = wanted
        .iter()
        .filter(|capability| !held.contains(capability))
        .cloned()
        .collect();
    if added.is_empty() {
        return Ok(GrantOutcome {
            held_before: held.len(),
            held_after: held.len(),
            added,
            expires_in: remaining,
            backup: None,
        });
    }
    if remaining <= 0 {
        return Err(deployment(format!(
            "the {consumer} grant expired {} seconds ago; re-mint it deliberately instead",
            -remaining
        )));
    }

    // A bearer this cannot reproduce is a bearer it must not replace: the
    // holders of the old one would start failing with no way back.
    if !token_file.is_file() {
        return Err(deployment(format!(
            "no bearer file at {}; refusing to re-mint {consumer}",
            token_file.display()
        )));
    }
    let bearer = std::fs::read_to_string(token_file).map_err(|error| {
        deployment(format!(
            "cannot read bearer file {}: {error}",
            token_file.display()
        ))
    })?;
    if hex::encode(Sha256::digest(bearer.trim().as_bytes())) != recorded_hash {
        return Err(deployment(format!(
            "{} does not hash to the bearer the vault recorded for {consumer}; refusing to \
             re-mint it, because the holders of the recorded bearer could not be given it back",
            token_file.display()
        )));
    }

    let backup = vault.with_extension(format!("before-{consumer}-{item}-grant.json"));
    std::fs::copy(&vault, &backup).map_err(|error| {
        deployment(format!(
            "cannot copy {} to {} before re-minting {consumer}: {error}",
            vault.display(),
            backup.display()
        ))
    })?;

    let union = held
        .iter()
        .chain(added.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    let output = std::process::Command::new(&binary)
        .arg("token-mint")
        .arg(consumer)
        .arg("--capabilities")
        .arg(&union)
        .arg("--token-file")
        .arg(token_file)
        .arg("--replace-capabilities")
        .arg("--ttl-seconds")
        .arg(remaining.to_string())
        .arg("--audience")
        .arg(&audience)
        .env("SKARBIEC_VAULT_FILE", &vault)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output()
        .map_err(|error| deployment(format!("cannot run {}: {error}", binary.display())))?;
    if !output.status.success() {
        return Err(deployment(format!(
            "skarbiec refused to widen the {consumer} grant: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let settled_document = read_vault(&vault)?;
    let settled = grant_of(&settled_document, consumer)?;
    let settled_hash = settled
        .get("hash")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if settled_hash != recorded_hash {
        return Err(deployment(format!(
            "the mint rotated the {consumer} bearer despite --token-file: every holder of the \
             previous bearer is now refused. The vault as it stood before is at {}",
            backup.display()
        )));
    }
    let settled_capabilities: Vec<String> = settled
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|list| list.iter().map(encode).collect())
        .unwrap_or_default();
    if let Some(missing) = wanted
        .iter()
        .find(|capability| !settled_capabilities.contains(capability))
    {
        return Err(deployment(format!(
            "the mint left {consumer} without {missing}; the vault as it stood before is at {}",
            backup.display()
        )));
    }
    Ok(GrantOutcome {
        held_before: held.len(),
        held_after: settled_capabilities.len(),
        added,
        expires_in: settled
            .get("expires_at")
            .and_then(Value::as_i64)
            .unwrap_or(expires_at)
            - now_seconds(),
        backup: Some(backup),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_field_and_whole_item_capabilities() {
        assert_eq!(
            encode(&json!({"action": "read", "item": "stado-ssh-x", "field": "private_key"})),
            "read:stado-ssh-x#private_key"
        );
        assert_eq!(
            encode(&json!({"action": "read", "item": "stado-ssh-x", "field": Value::Null})),
            "read:stado-ssh-x"
        );
        assert_eq!(
            encode(&json!({"action": "write", "item": "stado-ssh-x"})),
            "write:stado-ssh-x"
        );
    }

    #[test]
    fn missing_consumer_is_named() {
        let document = json!({"tokens": {"other": {}}});
        let error = grant_of(&document, "local-operator")
            .unwrap_err()
            .to_string();
        assert!(error.contains("local-operator"), "{error}");
    }
}
