//! What a rival is said to do, each statement bound to the page it was read
//! from.
//!
//! A comparison with a rival is only as good as its sources. Each claim names
//! the capability of ours it is compared with, the statement, the https page
//! it was read from, when it was read (`read_at`, RFC 3339) and the SHA-256
//! of the page's bytes as read (`source_sha256`); `verified_by` names the
//! Probierz run that exercised it, when one did. `stado product registry
//! check-claims <product>` fetches every source again: a page whose bytes
//! changed since it was read leaves its claim `changed`, so the statement is
//! read again before it is used, and a page that cannot be fetched is
//! `unreachable` with the cause. Only that claim is affected, never the rest
//! of the product's record.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::common::Runtime;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Claim {
    capability: String,
    statement: String,
    source: String,
    read_at: String,
    source_sha256: String,
    #[serde(default)]
    verified_by: Option<String>,
}

/// Every claim of rival `rival` of product `id`, refused naming the claim and
/// what it lacks.
pub(super) fn validate(id: &str, rival: &str, claims: &[Claim]) -> Result<()> {
    for claim in claims {
        let place = format!("{id}.rivals.{rival}.claims");
        if claim.capability.trim().is_empty() || claim.statement.trim().is_empty() {
            bail!("{place}: capability and statement must be non-empty");
        }
        if !claim.source.starts_with("https://") {
            bail!(
                "{place}: source must be an https page, got {}",
                claim.source
            );
        }
        let read = chrono::DateTime::parse_from_rfc3339(&claim.read_at).with_context(|| {
            format!("{place}: read_at {} is not an RFC 3339 time", claim.read_at)
        })?;
        if read > chrono::Utc::now() {
            bail!(
                "{place}: read_at {} is in the future; a source is read before it is cited",
                claim.read_at
            );
        }
        let digest = hex::decode(&claim.source_sha256).with_context(|| {
            format!(
                "{place}: source_sha256 {} is not hexadecimal",
                claim.source_sha256
            )
        })?;
        if digest.len() != <Sha256 as Digest>::output_size()
            || claim.source_sha256 != claim.source_sha256.to_lowercase()
        {
            bail!(
                "{place}: source_sha256 must be the lowercase SHA-256 of the page as read, got {}",
                claim.source_sha256
            );
        }
        if claim
            .verified_by
            .as_deref()
            .is_some_and(|run| run.trim().is_empty())
        {
            bail!("{place}: verified_by names the Probierz run that exercised the claim and must be non-empty when given");
        }
    }
    Ok(())
}

/// One claim's source fetched again: `current` when its bytes hash as read,
/// `changed` with the new digest, or `unreachable` with the cause.
fn checked(rival: &str, claim: &Claim) -> Value {
    let row = |state: &str, detail: Value| {
        json!({"rival": rival, "capability": claim.capability, "source": claim.source, "readAt": claim.read_at,
            "state": state, "detail": detail})
    };
    let fetched = reqwest::blocking::get(&claim.source)
        .and_then(reqwest::blocking::Response::error_for_status);
    let body = match fetched.and_then(reqwest::blocking::Response::bytes) {
        Ok(body) => body,
        Err(error) => return row("unreachable", json!(error.to_string())),
    };
    let observed = hex::encode(Sha256::digest(&body));
    match observed == claim.source_sha256 {
        true => row("current", Value::Null),
        false => row(
            "changed",
            json!(format!("the page now hashes to {observed}; read it again and restate the claim before it is used")),
        ),
    }
}

/// Every claim of every rival of `product`, each source fetched again.
pub fn check(runtime: &Runtime, product: &str) -> Result<(Value, bool)> {
    let text = std::fs::read_to_string(&runtime.catalog)
        .with_context(|| format!("the catalog {} cannot be read", runtime.catalog.display()))?;
    let document: Value = serde_yaml::from_str(&text)
        .with_context(|| format!("the catalog {} is not YAML", runtime.catalog.display()))?;
    let record = document["products"]
        .as_array()
        .context("products must be a list")?
        .iter()
        .find(|entry| entry["id"] == product)
        .with_context(|| format!("no product {product} in {}", runtime.catalog.display()))?;
    let mut rows = Vec::new();
    if let Some(rivals) = record.get("rivals") {
        for rival in rivals
            .as_array()
            .with_context(|| format!("{product}.rivals must be a list"))?
        {
            let id = rival["id"]
                .as_str()
                .with_context(|| format!("{product}.rivals: a rival names no id"))?;
            if let Some(claims) = rival.get("claims") {
                for claim in claims
                    .as_array()
                    .with_context(|| format!("{product}.rivals.{id}.claims must be a list"))?
                {
                    let claim = Claim::deserialize(claim).with_context(|| {
                        format!("{product}.rivals.{id}.claims: invalid claim {claim}")
                    })?;
                    rows.push(checked(id, &claim));
                }
            }
        }
    }
    let standing = rows.iter().all(|row| row["state"] == "current");
    Ok((
        json!({"product": product, "claims": rows, "allCurrent": standing}),
        standing,
    ))
}
