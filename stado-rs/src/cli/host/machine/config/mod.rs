//! `stado host config ...` — read and write one host config key.

pub(in crate::cli::host) mod guards;
pub(in crate::cli::host) mod remote;

use crate::cli::CmdError;

use crate::cli::host::machine::config::guards::{
    refuse_unminted_publisher, warn_unbacked_object_namespace, warn_unbacked_verifier_item,
};
use crate::cli::host::machine::config::remote::{
    remote_config, remote_config_output, RemoteConfigAction,
};

/// Read the effective configuration on a fleet host using the same installed
/// Stado binary and config path its services consume: `key: value` lines, or
/// the host's own JSON document with `--json`.
pub async fn config_show(target: &str, json: bool) -> Result<(), CmdError> {
    remote_config(target, RemoteConfigAction::Show, json).await
}

/// Persist one configuration field on a fleet host. Values travel base64
/// encoded inside the audited script and are decoded into argv, never parsed by
/// a remote shell. When `reload_service` is named, the existing service
/// reconciler activates the new configuration only after the atomic write
/// succeeds.
pub async fn config_set(
    target: &str,
    key: &str,
    value: &str,
    reload_service: Option<&str>,
) -> Result<(), CmdError> {
    let stdout = write_host_config(target, key, value).await?;
    print!("{stdout}");
    if let Some(service) = reload_service {
        crate::cli::service::reconcile_after_config_change(service, target).await?;
    }
    Ok(())
}

/// The guarded write itself, returning the host's resolved configuration
/// instead of printing it.
///
/// A caller that owns its own report cannot print this document: with
/// `--json` a second one on the same stream makes the answer unparseable.
/// The guards stay here so no writer can reach the host without them.
pub(crate) async fn write_host_config(
    target: &str,
    key: &str,
    value: &str,
) -> Result<String, CmdError> {
    if key.trim().is_empty() || key.chars().any(char::is_whitespace) {
        return Err(CmdError::usage(
            "configuration key must be a non-empty dotted name",
        ));
    }
    // Before the write, not after: a declaration whose item does not exist
    // closes the host's release publication boundary the moment the unit
    // reloads, and the cheapest place to say so is here.
    refuse_unminted_publisher(target, key, value).await?;
    let canonical = crate::cli::canonical_host(target).await?;
    let stdout = remote_config_output(
        &canonical,
        RemoteConfigAction::Set { key, value },
        &crate::deploy::production_runner(),
    )
    .await?;
    warn_unbacked_object_namespace(target, key, value);
    warn_unbacked_verifier_item(target, key, value);
    Ok(stdout)
}

/// Retract one configuration key from a fleet host.
///
/// Setting a declaration to `null` is not a retraction: a key present with a
/// null value and a key that is absent read alike through `jq` and differently
/// through the code that iterates the object, which is how a publisher nobody
/// meant to declare kept being counted.
pub async fn config_unset(
    target: &str,
    key: &str,
    reload_service: Option<&str>,
) -> Result<(), CmdError> {
    if key.trim().is_empty() || key.chars().any(char::is_whitespace) {
        return Err(CmdError::usage(
            "configuration key must be a non-empty dotted name",
        ));
    }
    let resolved = crate::cli::canonical_host(target).await?;
    let stdout = remote_config_output(
        &resolved,
        RemoteConfigAction::Unset { key },
        &crate::deploy::production_runner(),
    )
    .await?;
    print!("{stdout}");
    if let Some(service) = reload_service {
        crate::cli::service::reconcile_after_config_change(service, &resolved.name).await?;
    }
    Ok(())
}
