//! The last step of a vault move before its commit: the source host reads
//! the destination's vault the way its registry authority and object API
//! will right after the commit.
//!
//! The commit retargets the source's resolver to the destination at once,
//! and the authority checks every store request's bearer against the vault
//! it is then sent to. A destination that answered its own loopback probe but
//! nothing over the channel from the source took the authority's store down
//! with the commit. So the source's own Stado (`stado host local
//! vault-reach`) reads every object API bearer's revision from the
//! destination's declared vault endpoint, over the native SSH channel its
//! resolver opens: the session within the resolver's refresh interval and
//! each read within that interval less the session's time, since the object
//! API makes one read per request and the resolver gives each request up
//! alone. A source that cannot is refused here, before anything is committed,
//! and the move rolls back as for any step before the commit.
//!
//! The channel needs the source resolver's own SSH key authorized on the
//! destination: a source that is the registry authority never opened a
//! session to another host, so it held no resolver key, and its reach failed
//! with `no identity is available` while the vault on the destination was
//! up. The step before the reach mints that key on the source when it has
//! none and authorizes it on the destination ([`authorize_source`]), the same
//! operation as `stado route key <source> --to <destination>`.

use serde_json::Value;

use super::MoveContext;
use crate::cli::host::remote_stado_verdict;
use crate::cli::CmdError;
use crate::deploy::Runner;

/// Authorize the source resolver's SSH key on the destination, minting the
/// key on the source when it has none: the hop the source's resolver opens to
/// the vault once the move commits.
pub(super) async fn authorize_source(context: &MoveContext) -> Result<(), CmdError> {
    let registry = crate::targets::load_registry_from_str(
        &serde_json::to_string(&context.claimed_document)?,
    )
    .map_err(|error| CmdError::declaration(error.to_string()))?;
    crate::deploy::host_access::resolver_key::authorize(
        &registry,
        &context.source.name,
        Some(&context.destination.name),
    )
    .await
    .map(drop)
    .map_err(CmdError::from)
}

pub(super) async fn source_reads_destination(
    context: &MoveContext,
    runner: &Runner,
) -> Result<(), CmdError> {
    let source = &context.source.name;
    let destination = &context.destination.name;
    let vault = crate::cli::secrets::VAULT_SERVICE;
    let directory = crate::service_resolution::directory(&context.claimed_document)
        .map_err(CmdError::declaration)?
        .ok_or_else(|| CmdError::declaration("registry.service_directory is required"))?;
    let endpoint = directory
        .services
        .get(vault)
        .and_then(|route| route.endpoints.get(destination))
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "the service directory declares no {vault} endpoint on {destination}, so the \
                 address the resolver on {source} would send the vault's readers to cannot be read"
            ))
        })?;
    let output = remote_stado_verdict(
        &context.source,
        &[
            "host",
            "local",
            "vault-reach",
            "--host",
            destination,
            "--endpoint",
            &endpoint.url,
        ],
        runner,
    )
    .await?;
    let receipt: Value = serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{source}: `stado host local vault-reach` answered no receipt ({error}; exit {}): {} \
             — a Stado on {source} older than this step does not know the verb; install this \
             release there first",
            output.code,
            output.detail().trim()
        ))
    })?;
    if receipt["reached"].as_bool() == Some(true) {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "{source} did not read the vault at {} on {destination} over the channel its resolver \
         will use once the move commits: {}; reads: {}. Committing would leave the registry \
         authority and the object API on {source} checking their bearers against a vault they \
         cannot reach, so the move is not committed",
        endpoint.url,
        receipt["cause"].as_str().unwrap_or("no cause was recorded"),
        receipt["reads"]
    )))
}
