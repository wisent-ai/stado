//! `stado agent` — run the local GPU agent (polls queue, respects Vast.ai
//! renters). Port of the `agent` command in `stado/cli.py`.

use crate::cli::CmdError;
use crate::providers::local::agent as local_agent;
use crate::providers::vast;
use crate::queue::JobStorage;

/// Python `str(v)` for a registry env_overrides value: JSON strings pass
/// through; anything else uses its JSON rendering (Python's str() of
/// numbers/bools differs only for True/False/none — an accepted cosmetic
/// deviation for env values, which are strings in practice).
fn env_value_str(v: &serde_json::Value) -> String {
    v.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string())
}

/// The shared --auto/--target registry-application half of the Python
/// command. Returns the (possibly registry-supplied) gpu_type.
async fn apply_registry_target(
    mut gpu_type: String,
    target: Option<&str>,
    auto: bool,
) -> Result<String, CmdError> {
    if auto {
        let hostname = vast::system_hostname();
        let t = local_agent::lookup_self_auto(&hostname)
            .await
            .map_err(|e| CmdError::click(e.to_string()))?
            .ok_or_else(|| CmdError::click(format!("hostname '{hostname}' not in registry")))?;
        if gpu_type.is_empty() {
            gpu_type = t.gpu_type.clone().unwrap_or_default();
        }
        let env_slots = std::env::var("WC_LOCAL_SLOTS")
            .unwrap_or_default()
            .trim()
            .to_string();
        if t.slots > 0 || env_slots.is_empty() {
            std::env::set_var("WC_LOCAL_SLOTS", t.slots.to_string());
        }
        for (k, v) in &t.env_overrides {
            std::env::set_var(k, env_value_str(v));
        }
        let effective_slots =
            std::env::var("WC_LOCAL_SLOTS").unwrap_or_else(|_| t.slots.to_string());
        println!(
            "agent --auto: target={} gpu_type={gpu_type} slots={effective_slots} registry_slots={}",
            t.name, t.slots
        );
    } else if let Some(target) = target {
        let t = local_agent::lookup_auto(target)
            .await
            .map_err(|e| CmdError::click(e.to_string()))?
            .ok_or_else(|| CmdError::click(format!("target '{target}' not found in registry")))?;
        if !crate::capabilities::ProviderId::Local.matches(&t.kind) {
            return Err(CmdError::click(format!(
                "target '{target}' kind={}, expected local",
                t.kind
            )));
        }
        if gpu_type.is_empty() {
            gpu_type = t.gpu_type.clone().unwrap_or_default();
        }
        let env_slots = std::env::var("WC_LOCAL_SLOTS")
            .unwrap_or_default()
            .trim()
            .to_string();
        if t.slots > 0 || env_slots.is_empty() {
            std::env::set_var("WC_LOCAL_SLOTS", t.slots.to_string());
        }
        let effective_slots =
            std::env::var("WC_LOCAL_SLOTS").unwrap_or_else(|_| t.slots.to_string());
        println!(
            "agent: target={} gpu_type={gpu_type} slots={effective_slots} registry_slots={}",
            t.name, t.slots
        );
    }
    Ok(gpu_type)
}

/// Python the `agent` click command body.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    gpu_type: String,
    target: Option<String>,
    auto: bool,
    idle_shutdown: bool,
    kind: String,
    vast_auto_list: bool,
    vast_price_gpu: f64,
    vast_max_duration_s: i64,
) -> Result<(), CmdError> {
    let execution = crate::capabilities::configurable_variant(
        crate::capabilities::RuntimeFacet::Execution,
        &kind,
    )
    .ok_or_else(|| {
        let choices =
            crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Execution)
                .collect::<Vec<_>>()
                .join(", ");
        CmdError::usage(format!(
            "unknown agent kind {kind:?}; use one of: {choices}"
        ))
    })?;
    let kind = execution.id.to_string();
    let gpu_type = apply_registry_target(gpu_type, target.as_deref(), auto).await?;

    // Auto-enable the Vast bridge when stado-vast/api_key exists in
    // Skarbiec and this is a local consumer. The defensive helper performs
    // the authoritative lookup below. WC_VAST_AUTO_LIST remains an explicit
    // non-secret operator override.
    let auto_list_env = std::env::var("WC_VAST_AUTO_LIST")
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let explicit_off = matches!(auto_list_env.as_str(), "0" | "false" | "no" | "off");
    let explicit_on = matches!(auto_list_env.as_str(), "1" | "true" | "yes" | "on");
    let inference_reservation = crate::inference::reservation::active();
    let env_has_api_key = if inference_reservation.is_none() {
        vast::vast_api_key_available().await
    } else {
        false
    };
    let effective_vast = inference_reservation.is_none()
        && (vast_auto_list
            || explicit_on
            || (crate::capabilities::ProviderId::Local.matches(&kind)
                && env_has_api_key
                && !explicit_off));
    if let Some(reservation) = &inference_reservation {
        println!(
            "agent: Vast disabled by exclusive inference reservation '{}'",
            reservation.deployment
        );
    }
    if effective_vast {
        // Spawn the Vast.ai auto-listing daemon as a background task so
        // one `stado agent --vast-auto-list` invocation gives the operator both
        // the wisent-compute claim loop AND the Vast.ai marketplace toggle
        // in a single process — no separate systemd unit, no env-variable
        // plumbing across processes.
        //
        // Probe config eagerly so misconfiguration fails fast at agent
        // start instead of N seconds later inside the task.
        let client = vast::VastClient::from_env()
            .await
            .map_err(|e| CmdError::click(format!("vast bridge requested but {e}")))?;
        client
            .machine_id()
            .await
            .map_err(|e| CmdError::click(format!("vast bridge requested but {e}")))?;
        let store = JobStorage::new().await?;
        let hostname = vast::system_hostname();
        let params = vast::AutoListParams {
            price_gpu: vast_price_gpu,
            duration_s: (vast_max_duration_s > 0).then_some(vast_max_duration_s),
            ..Default::default()
        };
        tokio::spawn(async move {
            if let Err(exc) = vast::auto_list_loop(&client, &store, &hostname, params, |m| {
                println!("[vast] {m}")
            })
            .await
            {
                eprintln!("[vast] auto-list loop exited: {exc}");
            }
        });
        println!("[vast] auto-list thread started (price-gpu=${vast_price_gpu}/h)");
    }
    loop {
        match local_agent::run_agent(&gpu_type, idle_shutdown, &kind).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                local_agent::agent_log(&format!(
                    "agent loop failed: {error}; restarting after bounded delay"
                ));
                tokio::time::sleep(std::time::Duration::from_secs(
                    crate::constants::POLL_INTERVAL_S,
                ))
                .await;
            }
        }
    }
}
