//! Every consumer grant the service directory declares, kept live and bound
//! to its declared bearer by the host that holds the owner vault.
//!
//! A product reads its credentials under a consumer the directory declares
//! (`service_directory.services.<service>.consumers.<consumer>.grants`, with
//! the owner-only bearer file it reads on the service's active host), and
//! `stado service grant mint` issues those grants until revoked. A grant the
//! vault still records with an end - issued before that, or by hand with
//! `--ttl-seconds` - lapses on a clock, and a grant whose recorded bearer is no
//! longer the one in the declared file refuses every read; either way the
//! product is refused although nothing revoked it. When the service is placed
//! on this host, the grant is re-issued until revoked against the declared
//! file with the capabilities and audience it holds (`bearer`), so the end
//! goes and the grant is bound to the bearer the product presents; the file is
//! read by the vault, never rotated. When the service is placed elsewhere the
//! path is not read here, since it could name another file; a grant with an
//! end is kept by the vault (`skarbiec grant keep`: same capabilities, same
//! bearer, no end), and that the bearer is checked where it lives is said.
//! A grant no declaration owns keeps its end and is named, because only a
//! declaration says that a consumer is meant to keep reading.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

use crate::cli::secrets::{launcher_json, skarbiec_launcher};
use crate::credential_store::grant::GrantLifetime;

use super::bearer::Bound;

/// The sentences the last pass logged, so a pass that says the same on every
/// tick is logged once, and again when what it says changes.
static LAST_SENTENCES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What one pass over the owner vault's grants did.
#[derive(Debug, Default)]
pub(crate) struct KeepReport {
    /// Each grant kept until revoked, as `consumer`.
    pub(crate) kept: Vec<String>,
    /// Each grant re-bound to its declared bearer file, as `consumer: file`.
    pub(crate) rebound: Vec<String>,
    /// Each declared grant whose bearer file is not on this host, as
    /// `consumer: file`.
    pub(crate) elsewhere: Vec<String>,
    /// Each direct grant with an end that no declaration owns.
    pub(crate) undeclared: Vec<String>,
    /// Each grant a renewal or rebind failed for, with the cause.
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

/// A declared grant's bearer file and the host it lives on: the service's
/// active host, which the declaration names the file on.
struct Declaration {
    token_file: String,
    host: String,
}

/// Every consumer a declared grant names, with its bearer file and that
/// file's host; the first declaration of a consumer is the one read.
fn declared(registry: &crate::targets::Registry) -> BTreeMap<String, Declaration> {
    let mut files = BTreeMap::new();
    let Some(directory) = registry.service_directory.as_ref() else {
        return files;
    };
    for entry in directory.services.values() {
        for consumer in entry.consumers.values() {
            for grant in &consumer.grants {
                files
                    .entry(grant.consumer.clone())
                    .or_insert_with(|| Declaration {
                        token_file: grant.token_file.clone(),
                        host: entry.active_host.clone(),
                    });
            }
        }
    }
    files
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
    let hostname = crate::providers::vast::system_hostname();
    let here = registry
        .lookup_self(&hostname)
        .map_err(|error| {
            NotHere::Unreadable(format!(
                "this host's registry target cannot be told: {error}"
            ))
        })?
        .map(|target| target.name.clone())
        .ok_or_else(|| {
            NotHere::Unreadable(format!(
                "host {hostname:?} has no registry target, so which declared bearers live here \
                 cannot be told"
            ))
        })?;
    keep_in(&launcher, &vault, &here, &declared(&registry))
}

fn keep_in(
    launcher: &Path,
    vault: &Path,
    here: &str,
    declared: &BTreeMap<String, Declaration>,
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
        if workload_bound {
            continue;
        }
        let has_end = !GrantLifetime::is_until_revoked(expires_at);
        let Some(declaration) = declared.get(consumer) else {
            if has_end {
                report.undeclared.push(consumer.to_string());
            }
            continue;
        };
        // The declared path names a file on the service's host; read on any
        // other host it could be another file, whose bearer must not be bound.
        let file = crate::config_file::expand_tilde(&declaration.token_file);
        let local = declaration.host == here;
        if local && !file.is_file() {
            report.refused.push(format!(
                "{consumer}: its declared bearer file {} is not a file on {here}, the service's \
                 host, so its product has no bearer to present",
                file.display()
            ));
        }
        if !local {
            report.elsewhere.push(format!(
                "{consumer}: {} on {}",
                declaration.token_file, declaration.host
            ));
        }
        if !local || !file.is_file() {
            if has_end {
                match launcher_json(launcher, vault, &["grant", "keep", consumer]) {
                    Ok(_) => report.kept.push(consumer.to_string()),
                    Err(error) => report.refused.push(format!("{consumer}: {error}")),
                }
            }
            continue;
        }
        match super::bearer::bind(launcher, vault, consumer, grant, &file, has_end) {
            Ok(Bound::Already) => {}
            Ok(Bound::Kept) => report.kept.push(consumer.to_string()),
            Ok(Bound::Rebound) => report
                .rebound
                .push(format!("{consumer}: {}", file.display())),
            Err(cause) => report.refused.push(format!("{consumer}: {cause}")),
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
            .chain(report.rebound.iter().map(|bound| {
                format!(
                    "declared grants: re-bound {bound}: the declared bearer did not open the \
                     grant, so it was re-issued until revoked against that file with the \
                     capabilities and audience it held"
                )
            }))
            .chain(report.elsewhere.iter().map(|elsewhere| {
                format!(
                    "declared grants: the bearer of {elsewhere} is not on this host, so whether it \
                     still opens the grant is checked where it lives (stado credentials token \
                     sync --check)"
                )
            }))
            .chain(report.undeclared.iter().map(|consumer| {
                format!(
                    "declared grants: {consumer} has an end and no service directory declaration \
                     owns it, so it keeps its end"
                )
            }))
            .chain(report.refused.iter().map(|refusal| {
                format!("declared grants: renewing or binding the grant failed for {refusal}")
            }))
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
