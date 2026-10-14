//! The operator-facing verbs: [`validate`] and `import` of a local document,
//! [`push`] and [`pull`] of the canonical one, `self`, and the [`host`]
//! family. The local-file source both `validate` and `push` resolve, and
//! `self`, live here.

pub(in crate::cli::registry) mod host;
pub(in crate::cli::registry) mod pull;
pub(in crate::cli::registry) mod push;
pub(in crate::cli::registry) mod set;
pub(in crate::cli::registry) mod validate;

use crate::cli::registry::read_registry;
use crate::cli::CmdError;
use crate::targets;

/// `stado registry self [--name-only | --json] [--local]` — which registry
/// target is this machine. Installers need it: a plist that hardcodes a name
/// the registry does not carry produces a daemon that starts, fails its
/// identity lookup and exits, on every respawn, forever.
///
/// `--local` answers from this machine's own record and never asks the
/// registry authority: a local installation deciding whether a receipt's host
/// is this machine must not stand on another host that does not answer.
pub async fn self_target(name_only: bool, local: bool, json: bool) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let (registry, source) = if local {
        local_registry().await?
    } else {
        (read_registry().await?, targets::registry_location())
    };
    let found = registry.lookup_self(&hostname).map_err(CmdError::from)?;
    if json {
        let answer = serde_json::json!({
            "name": found.map(|target| &target.name),
            "kind": found.map(|target| target.kind.to_string()),
            "hostname": hostname,
            "source": source,
        });
        println!("{answer}");
        return Ok(());
    }
    let found = found.ok_or_else(|| {
        CmdError::click(format!("host {hostname} is not in {source}"))
            .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    if name_only {
        println!("{}", found.name);
    } else {
        println!("{}\t{}\t{}", found.name, found.kind, hostname);
    }
    Ok(())
}

/// This machine's own registry record and where it was read: the store
/// itself when it is a local directory (a file read, no other host is
/// asked), otherwise the last-known-good copy the last authority read left.
/// Refused naming the copy when there is none.
async fn local_registry() -> Result<(targets::Registry, String), CmdError> {
    if targets::store_is_local_filesystem() {
        return Ok((read_registry().await?, targets::registry_location()));
    }
    let copy = targets::registry_last_good_path().ok_or_else(|| {
        CmdError::click(
            "HOME is not set, so this machine's local registry record \
             (~/.stado/cache/registry-last-good.json) has no location",
        )
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let copy = copy.display().to_string();
    let (registry, _) = targets::last_good_for_this_host().ok_or_else(|| {
        CmdError::click(format!(
            "this machine keeps no usable local registry record: {copy} and its .meta.json \
             sidecar are not both there and readable; this host's resolver writes them on every \
             refresh, and `stado registry self` without --local writes them once the registry \
             authority answers"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    Ok((registry, copy))
}
