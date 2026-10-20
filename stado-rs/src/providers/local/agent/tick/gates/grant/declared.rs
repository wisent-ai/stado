//! Every consumer grant the service directory declares, kept until revoked by
//! the host that holds the owner vault.
//!
//! A product reads its credentials under a consumer the directory declares
//! (`service_directory.services.<service>.consumers.<consumer>.grants`), and
//! `stado service grant mint` issues those grants until revoked. A grant the
//! vault still records with an end - issued before that, or by hand with
//! `--ttl-seconds` - lapses on a clock, and from then every read the product
//! makes is refused although nothing revoked it. Re-issuing it here would
//! rotate a bearer that may live on another machine, so the vault is asked to
//! keep the grant it holds (`skarbiec grant keep`): same capabilities, same
//! bearer, no end. A grant no declaration owns keeps its end and is named,
//! because only a declaration says that a consumer is meant to keep reading.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

use crate::cli::secrets::{launcher_json, skarbiec_launcher};
use crate::credential_store::grant::GrantLifetime;

/// The sentences the last pass logged, so a pass that says the same on every
/// tick is logged once, and again when what it says changes.
static LAST_SENTENCES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What one pass over the owner vault's grants did.
#[derive(Debug, Default)]
pub(crate) struct KeepReport {
    /// Each grant kept until revoked, as `consumer`.
    pub(crate) kept: Vec<String>,
    /// Each direct grant with an end that no declaration owns.
    pub(crate) undeclared: Vec<String>,
    /// Each grant a keep was refused for, with the vault's sentence.
    pub(crate) refused: Vec<String>,
}

/// Why a pass did not look at any grant.
pub(crate) enum NotHere {
    /// This host holds no owner vault, or holds a replica of one: the vault's
    /// authority keeps its grants. The sentence says which.
    NotTheOwner(String),
    /// A read the pass needs failed; the sentence names it.
    Unreadable(String),
}

/// The consumer each declared grant names, with the `service/consumer` entry
/// of the directory that declares it.
fn declared(registry: &crate::targets::Registry) -> BTreeMap<String, String> {
    let mut owners = BTreeMap::new();
    let Some(directory) = registry.service_directory.as_ref() else {
        return owners;
    };
    for (service, entry) in &directory.services {
        for (authorized, consumer) in &entry.consumers {
            for grant in &consumer.grants {
                owners
                    .entry(grant.consumer.clone())
                    .or_insert_with(|| format!("{service}/{authorized}"));
            }
        }
    }
    owners
}

/// One row of `skarbiec grant list`: its consumer, its end, and whether it
/// is an acquire grant, which lends its lifetime to what it mints.
fn row(grant: &Value) -> Result<(&str, u64, bool), String> {
    let consumer = grant
        .get("consumer")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("a grant listed by the owner vault names no consumer: {grant}"))?;
    let expires_at = grant
        .get("expires_at")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("the owner vault lists {consumer} with no expires_at: {grant}"))?;
    let workload_bound = grant
        .get("workload_bound")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            format!("the owner vault lists {consumer} without workload_bound: {grant}")
        })?;
    Ok((consumer, expires_at, workload_bound))
}

/// Keep every declared grant the owner vault records with an end. A host
/// that is not the vault's authority does nothing and says why.
pub(crate) async fn keep_declared() -> Result<KeepReport, NotHere> {
    let vault = crate::credential_store::owner::vault().map_err(|error| {
        NotHere::NotTheOwner(format!("this host holds no owner vault: {error}"))
    })?;
    let launcher = skarbiec_launcher().map_err(|error| NotHere::Unreadable(error.to_string()))?;
    match super::vault::replica_of(&launcher, &vault) {
        Ok(Some(bond)) => {
            return Err(NotHere::NotTheOwner(format!(
                "{} replicates the vault of {bond}, whose own agent keeps its grants",
                vault.display()
            )))
        }
        Ok(None) => {}
        Err(error) => return Err(NotHere::Unreadable(error)),
    }
    let registry = crate::deploy::host_channel::canonical_registry()
        .await
        .map_err(|error| NotHere::Unreadable(format!("the registry cannot be read: {error}")))?;
    keep_in(&launcher, &vault, &declared(&registry))
}

fn keep_in(
    launcher: &Path,
    vault: &Path,
    declared: &BTreeMap<String, String>,
) -> Result<KeepReport, NotHere> {
    let listing = launcher_json(launcher, vault, &["grant", "list"]).map_err(|error| {
        NotHere::Unreadable(format!("the owner vault's grants cannot be read: {error}"))
    })?;
    let grants = listing.as_array().ok_or_else(|| {
        NotHere::Unreadable(format!("skarbiec grant list answered no array: {listing}"))
    })?;
    let mut report = KeepReport::default();
    for grant in grants {
        let (consumer, expires_at, workload_bound) = row(grant).map_err(NotHere::Unreadable)?;
        if workload_bound || GrantLifetime::is_until_revoked(expires_at) {
            continue;
        }
        if !declared.contains_key(consumer) {
            report.undeclared.push(consumer.to_string());
            continue;
        }
        match launcher_json(launcher, vault, &["grant", "keep", consumer]) {
            Ok(_) => report.kept.push(consumer.to_string()),
            Err(error) => report.refused.push(format!("{consumer}: {error}")),
        }
    }
    Ok(report)
}

/// The sentences one pass says, each naming the consumer it is about.
pub(crate) fn sentences(report: &Result<KeepReport, NotHere>) -> Vec<String> {
    match report {
        Err(NotHere::NotTheOwner(why)) => vec![format!("declared grants: nothing to keep here: {why}")],
        Err(NotHere::Unreadable(why)) => vec![format!("declared grants: {why}")],
        Ok(report) => report
            .kept
            .iter()
            .map(|consumer| {
                format!("declared grants: kept {consumer} until revoked, with its capabilities and bearer")
            })
            .chain(report.undeclared.iter().map(|consumer| {
                format!(
                    "declared grants: {consumer} has an end and no service directory declaration \
                     owns it, so it keeps its end"
                )
            }))
            .chain(
                report
                    .refused
                    .iter()
                    .map(|refusal| format!("declared grants: the vault refused to keep {refusal}")),
            )
            .collect(),
    }
}

/// The tick's pass: keep what is declared and log what changed since the
/// last pass. A host that is not the vault's authority logs nothing, because
/// there is nothing it could do.
pub(crate) async fn keep_if_due(log_fn: &mut dyn FnMut(&str)) {
    let report = keep_declared().await;
    if matches!(report, Err(NotHere::NotTheOwner(_))) {
        return;
    }
    let said = sentences(&report);
    let mut last = match LAST_SENTENCES.lock() {
        Ok(last) => last,
        // A pass that panicked while holding the list left whole sentences
        // in it; they are still the last ones logged.
        Err(poisoned) => poisoned.into_inner(),
    };
    if *last != said {
        for sentence in &said {
            log_fn(sentence);
        }
        *last = said;
    }
}
