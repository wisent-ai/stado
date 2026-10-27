//! The worker role of `stado serve --worker`: polls the queue against live
//! resources, respects Vast.ai renters, and on an ephemeral cloud machine
//! (`--idle-shutdown`) ends when no eligible work remains.

use crate::cli::CmdError;
use crate::providers::local::agent as local_agent;
use crate::providers::vast;
use crate::queue::JobStorage;

/// Python `str(v)` for a registry env_overrides value: JSON strings pass
/// through; anything else uses its JSON rendering (Python's str() of
/// numbers/bools differs only for True/False/none — an accepted cosmetic
/// deviation for env values, which are strings in practice).
fn env_value_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RegistryEnvironment {
    ResidentHost,
    /// Read host identity without applying a queue worker's environment.
    HostIdentity,
}

fn apply_environment(
    target: &crate::targets::ComputeTarget,
    environment: RegistryEnvironment,
) -> Result<(), CmdError> {
    if matches!(environment, RegistryEnvironment::HostIdentity) {
        return Ok(());
    }
    // Resolve every alias conflict before changing the shared environment.
    let overrides = target
        .env_overrides
        .iter()
        .map(|(key, value)| {
            let name = match environment {
                RegistryEnvironment::ResidentHost => {
                    crate::config::resident_worker_environment_key(key)
                }
                RegistryEnvironment::HostIdentity => key.as_str(),
            };
            let value = env_value_str(value);
            if name != key.as_str()
                && target
                    .env_overrides
                    .get(name)
                    .is_some_and(|other| env_value_str(other) != value)
            {
                return Err(CmdError::click(format!(
                    "target {} declares conflicting worker grant variables {key} and {name}",
                    target.name
                ))
                .stating(crate::primitives::failure::FailureCode::Config));
            }
            Ok((name, value))
        })
        .collect::<Result<Vec<_>, CmdError>>()?;
    for (name, value) in overrides {
        std::env::set_var(name, value);
    }
    Ok(())
}

/// The shared --auto/--target registry-application half of the Python
/// command. Returns the GPU type and the target already resolved.
pub(crate) async fn apply_registry_target(
    mut gpu_type: String,
    target: Option<&str>,
    auto: bool,
    environment: RegistryEnvironment,
) -> Result<(String, Option<crate::targets::ComputeTarget>), CmdError> {
    let mut resolved = None;
    if auto {
        let hostname = vast::system_hostname();
        let t = local_agent::lookup_self_auto(&hostname)
            .await
            .map_err(CmdError::from)?
            .ok_or_else(|| {
                CmdError::click(format!("hostname '{hostname}' not in registry"))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        if let (true, Some(declared)) = (gpu_type.is_empty(), &t.gpu_type) {
            gpu_type = declared.clone();
        }
        apply_environment(&t, environment)?;
        let capacity = match environment {
            RegistryEnvironment::HostIdentity => String::new(),
            _ => format!(" gpu_type={gpu_type} capacity=live-resources"),
        };
        println!("serve --auto: target={}{capacity}", t.name);
        resolved = Some(t);
    } else if let Some(target) = target {
        let t = local_agent::lookup_auto(target)
            .await
            .map_err(CmdError::from)?
            .ok_or_else(|| CmdError::refused(format!("target '{target}' not found in registry")))?;
        if !crate::capabilities::ProviderId::Local.matches(&t.kind) {
            return Err(CmdError::refused(format!(
                "target '{target}' kind={}, expected local",
                t.kind
            )));
        }
        if let (true, Some(declared)) = (gpu_type.is_empty(), &t.gpu_type) {
            gpu_type = declared.clone();
        }
        let capacity = match environment {
            RegistryEnvironment::HostIdentity => String::new(),
            _ => format!(" gpu_type={gpu_type} capacity=live-resources"),
        };
        println!("serve: target={}{capacity}", t.name);
        resolved = Some(t);
    }
    Ok((gpu_type, resolved))
}

/// The worker. `serve` resolved the host's registry target, GPU type and
/// environment before it started this role.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    gpu_type: String,
    idle_shutdown: bool,
    kind: String,
    vast_auto_list: bool,
    vast_price_gpu: Option<f64>,
    vast_price_disk: Option<f64>,
    vast_max_duration_s: Option<i64>,
    vast_idle_window_s: Option<i64>,
    poll_seconds: Option<u64>,
) -> Result<(), CmdError> {
    let poll = std::time::Duration::from_secs(poll_seconds.ok_or_else(|| {
        CmdError::usage(
            "the worker needs --poll-seconds: the seconds between queue polls when a poll \
             started nothing",
        )
    })?);
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
            "unknown worker kind {kind:?}; use one of: {choices}"
        ))
    })?;
    let kind = execution.id.to_string();

    // Auto-enable the Vast bridge when stado-vast/api_key exists in
    // Skarbiec and this is a local consumer. The defensive helper performs
    // the authoritative lookup below. WC_VAST_AUTO_LIST remains an explicit
    // non-secret operator override.
    let auto_list_env = match std::env::var("WC_VAST_AUTO_LIST") {
        Ok(value) => Some(value.trim().to_lowercase()),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => {
            return Err(CmdError::usage(format!(
                "WC_VAST_AUTO_LIST cannot be read: {error}"
            )))
        }
    };
    let explicit_off = matches!(auto_list_env.as_deref(), Some("0" | "false" | "no" | "off"));
    let explicit_on = matches!(auto_list_env.as_deref(), Some("1" | "true" | "yes" | "on"));
    // A reservation record that cannot be read may be one: Vast stays off,
    // and the cause is said once at agent start.
    let inference_reservation = crate::inference::reservation::active();
    if let Err(error) = &inference_reservation {
        eprintln!("agent: Vast disabled because the inference reservation cannot be read: {error}");
    }
    let reserved = !matches!(inference_reservation, Ok(None));
    // A reserved host lists no capacity, so its Vast key is not read at all.
    let env_has_api_key = !reserved && {
        let reading = vast::read_vast_api_key().await;
        // A channel that answered and refused is worth one line at agent
        // start; an absent channel is the ordinary state of a host that was
        // never meant to list capacity and says nothing.
        if let Some(error) = &reading.error {
            eprintln!("[vast] cannot read stado-vast/api_key: {error}");
        }
        reading.key.is_some()
    };
    let effective_vast = !reserved
        && (vast_auto_list
            || explicit_on
            || (crate::capabilities::ProviderId::Local.matches(&kind)
                && env_has_api_key
                && !explicit_off));
    if let Ok(Some(reservation)) = &inference_reservation {
        println!(
            "agent: Vast disabled by exclusive inference reservation '{}'",
            reservation.deployment
        );
    }
    if effective_vast {
        // Spawn the Vast.ai auto-listing daemon as a background task so
        // one `stado serve --worker --vast-auto-list` process carries both
        // the claim loop AND the Vast.ai marketplace toggle — no separate
        // unit, no env-variable plumbing across processes.
        //
        // Probe config eagerly so misconfiguration fails fast at agent
        // start instead of N seconds later inside the task.
        let bridge_refused = |error: vast::VastError| {
            let code = error.failure_code();
            let mut wrapped = CmdError::click(format!("vast bridge requested but {error}"));
            wrapped.failure = code;
            wrapped
        };
        let client = vast::VastClient::from_env().await.map_err(bridge_refused)?;
        client.machine_id().await.map_err(bridge_refused)?;
        let store = JobStorage::new().await?;
        let hostname = vast::system_hostname();
        let idle_window_s = vast_idle_window_s.ok_or_else(|| {
            CmdError::usage(
                "the Vast bridge needs --vast-idle-window-s: the seconds Stado must be idle \
                 before this host is listed",
            )
        })?;
        let price_gpu = vast_price_gpu.ok_or_else(|| {
            CmdError::usage("the Vast bridge needs --vast-price-gpu: the per-GPU-hour price in USD")
        })?;
        let price_disk = vast_price_disk.ok_or_else(|| {
            CmdError::usage(
                "the Vast bridge needs --vast-price-disk: the per-GB-month disk price in USD",
            )
        })?;
        let max_duration_s = vast_max_duration_s.ok_or_else(|| {
            CmdError::usage(
                "the Vast bridge needs --vast-max-duration-s: the longest rental in seconds, \
                 0 for open-ended",
            )
        })?;
        let params = vast::AutoListParams {
            idle_window_s,
            poll: Some(poll),
            price_gpu,
            price_disk,
            duration_s: max_duration_s.is_positive().then_some(max_duration_s),
            dry_run: false,
            once: false,
        };
        tokio::spawn(async move {
            if let Err(exc) = vast::auto_list_loop(Some(&client), &store, &hostname, params, |m| {
                println!("[vast] {m}")
            })
            .await
            {
                eprintln!("[vast] auto-list loop exited: {exc}");
            }
        });
        println!("[vast] auto-list thread started (price-gpu=${price_gpu}/h)");
    }
    // Poll host-local routines alongside fleet admission even while the fleet
    // store is unreachable. Neither future needs to migrate between tasks.
    let agent = local_agent::run_agent(&gpu_type, idle_shutdown, &kind, poll);
    tokio::pin!(agent);
    let result = tokio::select! {
        result = &mut agent => result,
        () = crate::providers::local::host_routines::run(poll) => agent.await,
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            let message = error.to_string();
            let mut refusal = format!("agent loop failed: {message}");
            // Namespace policy can refuse a required key even when the
            // bearer selected the correct namespace. Name that policy in
            // the hint rather than suggesting credential replacement.
            if message.contains("HTTP 401") {
                refusal.push_str(&format!(
                    "; the object API refused a key this binary uses — its \
                     object_api.namespaces.{} policy must grant every prefix in \
                     queue::copy::CANONICAL_PREFIXES; `stado doctor` on that host names the \
                     missing ones",
                    crate::config::QUEUE_OBJECT_NAMESPACE
                ));
            }
            Err(CmdError::click(refusal))
        }
    }
}
