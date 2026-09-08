use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::recovery::verifier::reconcile::reconcile_verifier;
use crate::cli::host::checks::recovery::verifier::release_publisher_items;
use crate::cli::host::machine::config::remote::{remote_config_output, RemoteConfigAction};

fn ensure_release_verifier_declarations_match(
    host: &BTreeMap<String, String>,
    local: &BTreeMap<String, String>,
) -> Result<(), CmdError> {
    let missing = host
        .iter()
        .filter(|(product, item)| local.get(*product) != Some(*item))
        .map(|(product, item)| format!("{product}={item}"))
        .collect::<Vec<_>>();
    let unexpected = local
        .iter()
        .filter(|(product, item)| host.get(*product) != Some(*item))
        .map(|(product, item)| format!("{product}={item}"))
        .collect::<Vec<_>>();
    if missing.is_empty() && unexpected.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "release_verifier_reconcile_declaration_mismatch: local release_api.publishers cannot \
         prove TARGET's declaration (missing_local=[{}], unexpected_local=[{}]); copy the \
         host's exact publisher declarations locally before reconciling",
        missing.join(","),
        unexpected.join(",")
    )))
}

/// Apply the declared release-verifier repair.
pub(crate) async fn apply_release_verifier_repair(target: &str) -> Result<Value, CmdError> {
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        CmdError::click(format!(
            "invalid release_api.publishers: {}",
            problems.join("; ")
        ))
    })?;
    let local = publishers
        .iter()
        .map(|(product, publisher)| (product.clone(), publisher.item().to_string()))
        .collect::<BTreeMap<_, _>>();
    let items = local.values().cloned().collect::<BTreeSet<_>>();
    let canonical = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let stdout = remote_config_output(
        &canonical,
        RemoteConfigAction::Show,
        &crate::deploy::production_runner(),
    )
    .await?;
    let document: Value = serde_json::from_str(&stdout).map_err(|error| {
        CmdError::click(format!(
            "release_verifier_reconcile_host_declaration_unreadable: {error}"
        ))
    })?;
    let host = release_publisher_items(&document)?;
    ensure_release_verifier_declarations_match(&host, &local)?;
    reconcile_verifier(
        target,
        "release",
        "matching local and target release_api.publishers",
        crate::config::RELEASE_API_VERIFIER_CONSUMER,
        "WC_RELEASE_SKARBIEC_TOKEN_FILE",
        "stado-release-api-verifier-skarbiec-token",
        items,
        true,
    )
    .await
}

/// Apply the declared service-verifier repair.
pub(crate) async fn apply_service_verifier_repair(target: &str) -> Result<Value, CmdError> {
    let deployers = crate::config::service_api_deployers().map_err(|problems| {
        CmdError::click(format!(
            "invalid service_api.deployers: {}",
            problems.join("; ")
        ))
    })?;
    let items = deployers
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    reconcile_verifier(
        target,
        "service",
        "service_api.deployers",
        crate::config::SERVICE_API_VERIFIER_CONSUMER,
        "WC_SERVICE_SKARBIEC_TOKEN_FILE",
        "stado-service-api-verifier-skarbiec-token",
        items,
        true,
    )
    .await
}

/// Read one nonsecret Skarbiec metadata report on a managed host.
///
/// Verifier reconciliation needs grant expiry, vault ownership and item lifecycle,
/// not the encrypted vault envelope. Reading those through Skarbiec keeps the
/// operator boundary intact and avoids transporting the whole vault over the host
/// channel.
pub(super) async fn remote_skarbiec_metadata(
    target: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
    skarbiec: &str,
    vault: &str,
    gnupg_home: &str,
    command: &str,
) -> Result<Value, CmdError> {
    let path = "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";
    let vault_environment = format!("SKARBIEC_VAULT_FILE={vault}");
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let output = crate::deploy::host_channel::run_program(
        target,
        &[
            "/usr/bin/env",
            path,
            gnupg_environment.as_str(),
            vault_environment.as_str(),
            skarbiec,
            command,
        ],
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec {command} metadata unavailable: {}",
            target.name,
            crate::deploy::host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: Skarbiec {command} returned unreadable metadata: {error}",
            target.name
        ))
    })
}
