//! `stado host config ...` — read and write one host config key.

pub(in crate::cli::host) mod guards;
pub(in crate::cli::host) mod remote;

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::cli::CmdError;

use crate::cli::host::machine::config::guards::{
    refuse_unminted_publisher, warn_unbacked_object_namespace, warn_unbacked_verifier_item,
};
use crate::cli::host::machine::config::remote::{
    remote_config, remote_config_output, RemoteConfigAction,
};

/// Read the effective configuration on a fleet host using the same installed
/// Stado binary and config path its services consume.
pub async fn config_show(target: &str) -> Result<(), CmdError> {
    remote_config(target, RemoteConfigAction::Show).await
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
        return Err(CmdError::click(
            "configuration key must be a non-empty dotted name",
        ));
    }
    // Before the write, not after: a declaration whose item does not exist
    // closes the host's release publication boundary the moment the unit
    // reloads, and the cheapest place to say so is here.
    refuse_unminted_publisher(target, key, value).await?;
    let canonical = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
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
        return Err(CmdError::click(
            "configuration key must be a non-empty dotted name",
        ));
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let script = format!(
        "{}\
         key=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
         \"$binary\" config unset \"$key\"\n\
         \"$binary\" config show\n",
        remote::CONFIG_SCRIPT_PREFIX,
        STANDARD.encode(key.as_bytes())
    );
    let output = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &script,
        std::time::Duration::from_secs(60),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        // The host's own sentence, not just its last line: `stado config
        // unset` prints why it refused and a tracing banner after it, so
        // reporting the last line reports the banner and loses the reason.
        let detail = output.detail().trim().to_string();
        return Err(CmdError::click(if detail.is_empty() {
            "remote Stado configuration command failed".to_string()
        } else {
            detail
        }));
    }
    print!("{}", output.stdout);
    if let Some(service) = reload_service {
        crate::cli::service::reconcile_after_config_change(service, &resolved.name).await?;
    }
    Ok(())
}
