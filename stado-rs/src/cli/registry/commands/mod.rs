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

/// `stado registry self [--name-only]` — which registry target is this
/// machine. Installers need it: a plist that hardcodes a name the registry
/// does not carry produces a daemon that starts, fails its identity lookup
/// and exits, on every respawn, forever.
pub async fn self_target(name_only: bool) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = read_registry().await?;
    let found = registry
        .lookup_self(&hostname)
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "host {hostname} is not in {}",
                targets::registry_location()
            ))
        })?;
    if name_only {
        println!("{}", found.name);
    } else {
        println!("{}\t{}\t{}", found.name, found.kind, hostname);
    }
    Ok(())
}
