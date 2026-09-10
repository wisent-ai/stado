use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::machine::config::remote::{remote_config_output, RemoteConfigAction};
use crate::cli::host::machine::config::write_host_config;

/// Restore the core object API without depending on the API being available.
///
/// Storage authority changes belong to the resident storage-root transaction.
/// This narrower repair shares its lock and never copies a backing store.
async fn recover_object_api_on_target(
    resolved: &ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let script = format!(
        r#"set -eu
/usr/bin/python3 - <<'PY'
import base64, fcntl, os, subprocess, sys
work = os.path.join(os.path.expanduser("~"), ".stado", "recovery")
os.makedirs(work, mode=0o700, exist_ok=True)
descriptor = os.open(
    os.path.join(work, "storage-root-reconcile.lock"),
    os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW,
    0o600,
)
with os.fdopen(descriptor, "a") as lock:
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        raise SystemExit("storage authority recovery is already running on this host")
    result = subprocess.run(
        ["/bin/bash"],
        input=base64.b64decode("{}"),
        pass_fds=(lock.fileno(),),
        check=False,
    )
    sys.exit(result.returncode)
PY"#,
        STANDARD.encode(include_str!(
            "../../../../../../deploy/recover_object_api.sh"
        )),
    );
    let recovered = crate::deploy::host_channel::run_script_with_timeout(
        resolved,
        &script,
        std::time::Duration::from_secs(300),
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !recovered.ok() {
        return Err(CmdError::click(format!(
            "{}: object API recovery failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&recovered, "remote command failed")
        )));
    }
    Ok(recovered.stdout.trim().to_string())
}

/// Apply the declared object-API repair.
pub(crate) async fn apply_object_api_repair(target: &str) -> Result<Value, CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let detail =
        recover_object_api_on_target(&resolved, &crate::deploy::production_runner()).await?;
    Ok(json!({
        "target": resolved.name,
        "healthy": true,
        "detail": detail,
    }))
}

/// Apply the declared release-catalog ownership repair.
pub(crate) async fn apply_release_store_repair(
    target: &str,
    product: &str,
) -> Result<Value, CmdError> {
    if !crate::release_control::identifier(product) {
        return Err(CmdError::usage(
            "product must be a canonical release identifier",
        ));
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let script = format!(
        "export STADO_RELEASE_STORE_PRODUCT={}\n{}",
        crate::deploy::shlex_quote(product),
        include_str!("../../../../../../deploy/release/repair_release_store.sh")
    );
    let repaired = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &script,
        std::time::Duration::from_secs(60),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !repaired.ok() {
        return Err(CmdError::click(format!(
            "{}: release store repair failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&repaired, "remote command failed")
        )));
    }
    let detail = repaired.stdout.trim();
    Ok(json!({
        "target": resolved.name,
        "status": "repaired",
        "scope": format!("stado://system/release-catalog/{product}.json"),
        "detail": detail,
    }))
}

/// Apply the agent's declared Skarbiec endpoint repair.
///
/// Derived, never invented: the value comes from
/// `service_directory.services.skarbiec.endpoints[<target>]`, and a host the
/// directory gives no endpoint is refused rather than pointed at a guess.
pub(crate) async fn apply_agent_skarbiec_repair(target: &str) -> Result<Value, CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let canonical = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let declared = document
        .pointer("/service_directory/services/skarbiec/endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(&canonical.name))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "the service directory declares no skarbiec endpoint for {}, so this host has \
                 no credential broker of its own to name. Declare one with `stado service \
                 directory endpoint skarbiec --target {} --url <url>`, or leave \
                 agent.skarbiec.url unset so the agent reads through the configured store \
                 client",
                canonical.name, canonical.name
            ))
        })?
        .to_string();

    let runner = crate::deploy::production_runner();
    let stdout = remote_config_output(&canonical, RemoteConfigAction::Show, &runner).await?;
    let current = serde_json::from_str::<Value>(&stdout)
        .ok()
        .and_then(|config| {
            config
                .pointer("/resolved/agent_skarbiec_url")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();

    let changed = current.trim() != declared;
    if changed {
        write_host_config(&canonical.name, "agent.skarbiec.url", &declared).await?;
    }
    Ok(json!({
        "target": canonical.name,
        "declared": declared,
        "previous": if current.trim().is_empty() { Value::Null } else { Value::from(current.trim()) },
        "changed": changed,
    }))
}
