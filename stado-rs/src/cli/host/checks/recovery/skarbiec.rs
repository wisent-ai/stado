use serde_json::{json, Value};

use crate::cli::CmdError;

/// Apply the declared Skarbiec audit-lock repair.
pub(crate) async fn apply_skarbiec_audit_repair(target: &str) -> Result<Value, CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let probes = target_health_probes(&resolved.name).await;
    let script = format!(
        "{probes}{}",
        include_str!("../../../../host_payloads/recover-skarbiec-audit-lock.sh")
    );
    let recovered = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &script,
        std::time::Duration::from_secs(90),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !recovered.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec audit recovery failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&recovered, "remote command failed")
        )));
    }
    let detail = recovered.stdout.trim();
    Ok(json!({
        "target": resolved.name,
        "recovered": detail.contains("recovered"),
        "detail": detail,
    }))
}

/// Shell prologue exporting the health endpoints TARGET itself serves on, read
/// from the registry's service directory.
///
/// The directory keys every endpoint by the asking machine because these
/// services bind loopback on their own host, so "where is Skarbiec" has a
/// different true answer on every machine. A helper that runs ON the target is
/// the target asking, which is why the lookup is `address_for(target)` and not
/// this laptop's own row.
///
/// Missing rows export nothing and leave the script's defaults alone: a
/// recovery that cannot name the endpoint should refuse on the endpoint it
/// documents rather than on one this function invented.
async fn target_health_probes(target: &str) -> String {
    let Ok(registry) = crate::cli::registry::read_registry().await else {
        return String::new();
    };
    let Some(directory) = registry.service_directory.as_ref() else {
        return String::new();
    };
    let mut prologue = String::new();
    for (service, variable, path) in [
        ("skarbiec", "SKARBIEC_HEALTH_URL", "/health"),
        ("stado-object-api", "STADO_OBJECT_HEALTH_URL", "/healthz"),
    ] {
        let Some(url) = directory
            .services
            .get(service)
            .and_then(|entry| entry.address_for(target))
            .map(|endpoint| endpoint.url.trim_end_matches('/').to_string())
        else {
            continue;
        };
        prologue.push_str(&format!(
            "{variable}=${{{variable}:-{}}}\nexport {variable}\n",
            crate::deploy::shlex_quote(&format!("{url}{path}"))
        ));
    }
    prologue
}

/// Apply the declared Skarbiec cryptographic-daemon repair.
pub(crate) async fn apply_skarbiec_crypto_repair(target: &str) -> Result<Value, CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let recovered = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        include_str!("../../../../host_payloads/recover-skarbiec-crypto.sh"),
        std::time::Duration::from_secs(240),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !recovered.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec cryptographic recovery failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&recovered, "remote command failed")
        )));
    }
    let detail = recovered.stdout.trim();
    Ok(json!({
        "target": resolved.name,
        "recovered": detail.contains("recovered"),
        "detail": detail,
    }))
}

/// Apply the declared Skarbiec acquisition-state repair.
pub(crate) async fn apply_skarbiec_acquisition_repair(target: &str) -> Result<Value, CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let recovered = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        include_str!("../../../../host_payloads/recover-skarbiec-acquisition-state.sh"),
        std::time::Duration::from_secs(90),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !recovered.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec acquisition-state recovery failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&recovered, "remote command failed")
        )));
    }
    let detail = recovered.stdout.trim();
    Ok(json!({
        "target": resolved.name,
        "recovered": detail.contains("recovered"),
        "detail": detail,
    }))
}
