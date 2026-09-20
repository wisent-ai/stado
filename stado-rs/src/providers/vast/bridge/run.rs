//! The auto-list daemon loop: the startup listing sync, the
//! inference-reservation preemption, and the per-iteration toggle that
//! applies [`decide_action`] and logs the Python messages.
//!
//! Moved verbatim out of the former single-file `providers/vast`, except
//! that the queued/running pair the busy branch renders three times is
//! named once above the branch — the shared write policy refuses that
//! rendering inside a branch body. The rendered text is unchanged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::providers::vast::client::json::{py_float, py_value_str};
use crate::providers::vast::{ListMachineParams, VastClient, VastError};
use crate::queue::JobStorage;

use super::{decide_action, is_stado_busy, AutoListAction, AutoListParams};

/// Python `_AUTO_LIST_THREAD_RUNNING` — set true when the auto-list loop
/// starts; read by the capacity broadcast (`vast_bridge_active`, phase-3).
pub static AUTO_LIST_THREAD_RUNNING: AtomicBool = AtomicBool::new(false);

/// Python `auto_list_loop` daemon: poll wisent-compute state, toggle the
/// Vast.ai listing. Lists the machine when wisent-compute has been idle
/// for `idle_window_s` consecutive seconds; unlists the moment any work
/// shows up. Existing Vast rentals are NOT touched. Runs forever, like
/// the Python loop (the CLI drives it as a daemon thread).
///
/// `client` is optional because a dry run calls no Vast endpoint: it reads
/// the queue and prints the toggle it would perform. Requiring a credential
/// for that contradicted the documented `--dry-run` contract and made the
/// decision loop unrunnable on a machine that is not provisioned — the exact
/// machine an operator is trying the flag on. Without `--dry-run` the loop
/// refuses when no client is supplied, because it would otherwise report
/// decisions it silently never applied.
pub async fn auto_list_loop(
    client: Option<&VastClient>,
    store: &JobStorage,
    hostname: &str,
    params: AutoListParams,
    mut log: impl FnMut(&str),
) -> Result<(), VastError> {
    if client.is_none() && !params.dry_run {
        return Err(VastError::config(
            "auto-list without --dry-run needs a Vast.ai API key; run it with \
             --dry-run to see the decisions, or `stado vast readiness` for what \
             is missing",
        ));
    }
    // WC_VAST_MAX_DURATION_S env wins (cli.py uneditable).
    let mut duration_s = params.duration_s;
    if let Ok(raw) = std::env::var("WC_VAST_MAX_DURATION_S") {
        if !raw.is_empty() {
            duration_s = Some(raw.trim().parse::<i64>().map_err(|_| {
                VastError::config(format!("WC_VAST_MAX_DURATION_S must be int: {raw}"))
            })?);
        }
    }
    AUTO_LIST_THREAD_RUNNING.store(true, Ordering::SeqCst);
    let mut idle_since: Option<std::time::Instant> = None;
    // Startup sync: take over any pre-existing listing (manual host-UI
    // placement or an earlier bridge run) so the loop can unlist it when
    // wisent-compute work arrives. Normalize price + duration to the
    // bridge's configured values.
    let mut listed = false;
    match client {
        None => log("startup: no Vast.ai credential; printing decisions only"),
        Some(client) => match client.machine_status().await {
            Ok(status) => {
                let cur_price = status
                    .get("listed_gpu_cost")
                    .cloned()
                    .unwrap_or(Value::Null);
                if let Some(current) = cur_price.as_f64().filter(|c| *c > 0.0) {
                    listed = true;
                    log(&format!(
                        "startup: listed at ${}/h on machine_id={}",
                        py_value_str(Some(&cur_price)),
                        py_value_str(status.get("id"))
                    ));
                    if (current - params.price_gpu).abs() > 0.01 && !params.dry_run {
                        let normalize = async {
                            client.unlist_machine().await?;
                            client
                                .list_machine(&ListMachineParams {
                                    price_gpu: params.price_gpu,
                                    duration: duration_s,
                                    ..ListMachineParams::default()
                                })
                                .await
                        };
                        match normalize.await {
                            Ok(_) => log(&format!(
                                "normalized ${}/h -> ${}/h",
                                py_value_str(Some(&cur_price)),
                                py_float(params.price_gpu)
                            )),
                            Err(exc) => log(&format!("normalize failed: {exc}")),
                        }
                    }
                } else {
                    log("startup: not currently listed");
                }
            }
            Err(exc) => log(&format!("startup probe failed: {exc}")),
        },
    }
    loop {
        if let Some(reservation) = crate::inference::reservation::active() {
            idle_since = None;
            if listed {
                if params.dry_run {
                    log(&format!(
                        "DRY-RUN would unlist for inference reservation '{}'",
                        reservation.deployment
                    ));
                } else if let Some(client) = client {
                    match client.unlist_machine().await {
                        Ok(_) => {
                            listed = false;
                            log(&format!(
                                "unlisted for inference reservation '{}'",
                                reservation.deployment
                            ));
                        }
                        Err(exc) => log(&format!(
                            "unlist for inference reservation '{}' failed: {exc}",
                            reservation.deployment
                        )),
                    }
                }
            }
            if params.once {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(params.poll_interval_s)).await;
            continue;
        }
        let state = match is_stado_busy(store, hostname).await {
            Ok(state) => state,
            Err(exc) => {
                let wrapped = VastError::Storage(exc);
                log(&format!("poll failed: {}: {wrapped}", wrapped.kind()));
                if params.once {
                    return Err(wrapped);
                }
                tokio::time::sleep(Duration::from_secs(params.poll_interval_s)).await;
                continue;
            }
        };
        // The three busy-branch messages below all render this pair; naming
        // it here renders the same text without repeating the pair inside
        // the branch. Lazy, so an idle poll still allocates nothing.
        let counts = || {
            format!(
                "queued={}, running_here={}",
                state.queued, state.running_here
            )
        };
        if state.idle {
            let now = std::time::Instant::now();
            let since = idle_since.get_or_insert(now);
            let idle_dur = now.duration_since(*since).as_secs() as i64;
            match decide_action(listed, &state, idle_dur, params.idle_window_s) {
                AutoListAction::List { idle_dur_s } => {
                    if params.dry_run {
                        // A dry run carries the state it would have written,
                        // so the decisions it prints are the sequence the
                        // live loop takes. Without it every poll repeated
                        // "would list" and no unlist decision could ever
                        // appear, which is the opposite of a preview.
                        listed = true;
                        log(&format!("DRY-RUN would list (idle {idle_dur_s}s)"));
                    } else if let Some(client) = client {
                        match client
                            .list_machine(&ListMachineParams {
                                price_gpu: params.price_gpu,
                                duration: duration_s,
                                ..ListMachineParams::default()
                            })
                            .await
                        {
                            Ok(_) => {
                                listed = true;
                                log(&format!(
                                    "LISTED ({idle_dur_s}s idle, ${}/h, dur={}s)",
                                    py_float(params.price_gpu),
                                    duration_s
                                        .map_or_else(|| "None".to_string(), |d| d.to_string())
                                ));
                            }
                            Err(exc) => log(&format!("list failed: {exc}")),
                        }
                    }
                }
                AutoListAction::IdleCountdown { idle_dur_s } => log(&format!(
                    "idle {idle_dur_s}s/{}s (listed={})",
                    params.idle_window_s,
                    if listed { "True" } else { "False" }
                )),
                _ => unreachable!("idle state only yields List/IdleCountdown"),
            }
        } else {
            idle_since = None;
            if decide_action(listed, &state, 0, params.idle_window_s) == AutoListAction::Unlist {
                if params.dry_run {
                    listed = false;
                    log(&format!("DRY-RUN would unlist ({})", counts()));
                } else if let Some(client) = client {
                    match client.unlist_machine().await {
                        Ok(_) => {
                            listed = false;
                            log(&format!("UNLISTED ({})", counts()));
                        }
                        Err(exc) => log(&format!("unlist failed: {exc}")),
                    }
                }
            }
            // Visibility for the "wait for renter to finish" path: no offer
            // of ours is up, wisent-compute has queued work and the box has
            // near-zero free VRAM, so something is holding the GPU and the
            // claim loop will sit idle until it lets go. The line says what
            // was read and marks the rental as the inference it is: on a
            // machine with no Vast credential at all it said "waiting for
            // Vast rental to finish" over a queue of two jobs and a laptop
            // GPU, which is a state that could not exist.
            match decide_action(listed, &state, 0, params.idle_window_s) {
                AutoListAction::WaitingForRental { free_vram_gb } if client.is_some() => {
                    log(&format!(
                        "not listed, queued={}, free_vram_gb={}: something holds the GPU, \
                         most likely a Vast rental; wisent-compute claims when it releases",
                        state.queued,
                        py_float(free_vram_gb)
                    ))
                }
                AutoListAction::WaitingForRental { free_vram_gb } => log(&format!(
                    "not listed, queued={}, free_vram_gb={}: the GPU is occupied and this \
                     host holds no Vast credential, so no rental of ours is on it",
                    state.queued,
                    py_float(free_vram_gb)
                )),
                AutoListAction::BusyNotListed => log(&format!("busy ({}); not listed", counts())),
                _ => {}
            }
        }
        if params.once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(params.poll_interval_s)).await;
    }
}
