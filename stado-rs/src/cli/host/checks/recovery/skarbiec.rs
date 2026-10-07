use serde_json::{json, Value};

use crate::cli::CmdError;

/// The exit status a recovery payload uses for "the host was healthy and
/// nothing was changed". 0 means the payload recovered; anything else is a
/// refusal or a failure. The verdict is read from the status, never from the
/// sentence.
const NOTHING_TO_RECOVER: i32 = 3;

/// Run one recovery payload on `target` and answer what it did. `probes`
/// prepends the health endpoints the target serves on, for a payload that
/// reads them.
async fn run_recovery(
    target: &str,
    payload: &str,
    probes: bool,
    what: &str,
) -> Result<Value, CmdError> {
    let resolved = crate::cli::canonical_host(target).await?;
    let runner = crate::deploy::production_runner();
    let script = match probes {
        true => format!("{}{payload}", target_health_probes(&resolved.name).await),
        false => payload.to_string(),
    };
    let ran = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(CmdError::from)?;
    let recovered = match ran.code {
        0 => true,
        NOTHING_TO_RECOVER => false,
        _ => {
            return Err(CmdError::click(format!(
                "{}: Skarbiec {what} recovery failed: {}",
                resolved.name,
                crate::deploy::host_channel::last_error_line(&ran, "remote command failed")
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown))
        }
    };
    Ok(json!({
        "target": resolved.name,
        "recovered": recovered,
        "detail": ran.stdout.trim(),
    }))
}

/// Apply the declared Skarbiec audit-lock repair.
pub(crate) async fn apply_skarbiec_audit_repair(target: &str) -> Result<Value, CmdError> {
    run_recovery(
        target,
        include_str!("../../../../host_payloads/recover-skarbiec-audit-lock.sh"),
        true,
        "audit",
    )
    .await
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
/// Missing rows export nothing: the script then reads the host's own forward
/// markers, and refuses naming the marker when that is missing too, rather
/// than probing an endpoint this function invented.
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
    run_recovery(
        target,
        include_str!("../../../../host_payloads/recover-skarbiec-crypto.sh"),
        false,
        "cryptographic",
    )
    .await
}

/// Apply the declared Skarbiec acquisition-state repair.
pub(crate) async fn apply_skarbiec_acquisition_repair(target: &str) -> Result<Value, CmdError> {
    run_recovery(
        target,
        include_str!("../../../../host_payloads/recover-skarbiec-acquisition-state.sh"),
        false,
        "acquisition-state",
    )
    .await
}
