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

/// This machine's own registry record and where it was read, through
/// [`targets::registry_without_authority`]: the store itself when it is a
/// local directory, otherwise the last-known-good copy the last authority
/// read left. Refused naming the copy and what writes it when there is none.
async fn local_registry() -> Result<(targets::Registry, String), CmdError> {
    match targets::registry_without_authority("naming this machine").await {
        Ok((registry, None)) => Ok((registry, targets::registry_location())),
        Ok((registry, Some(notice))) => {
            eprintln!("{notice}");
            let copy = targets::registry_last_good_path().ok_or_else(|| {
                CmdError::click("a registry copy was read and HOME no longer names where it is")
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
            Ok((registry, copy.display().to_string()))
        }
        Err(targets::RegistryFetchError::Absent { location })
            if !targets::store_is_local_filesystem() =>
        {
            Err(CmdError::click(format!(
                "this machine keeps no usable local registry record: {location} and its \
                 .meta.json sidecar are not both there and readable; this host's resolver writes \
                 them on every refresh, and `stado registry self` without --local writes them \
                 once the registry authority answers"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound))
        }
        Err(other) => Err(CmdError::from(other)),
    }
}
