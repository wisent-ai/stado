//! The auto-list daemon loop: the startup listing sync, the
//! inference-reservation preemption, and the per-iteration toggle that
//! applies [`decide_action`] and logs the Python messages.
//!
//! Moved verbatim out of the former single-file `providers/vast`, except
//! that the queued/running pair the busy branch renders three times is
//! named once above the branch — the shared write policy refuses that
//! rendering inside a branch body. The rendered text is unchanged.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::providers::vast::client::json::{py_float, shown};
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
    if params.poll.is_none() && !params.once {
        return Err(VastError::config(
            "auto-list needs --poll-interval-s, the seconds between queue polls, unless it \
             runs --once",
        ));
    }
    if params.poll.is_some_and(|poll| poll.is_zero()) {
        return Err(VastError::config(
            "auto-list's --poll-interval-s is zero; the seconds between queue polls must be \
             a positive number",
        ));
    }
    if client.is_none() && !params.dry_run {
        return Err(VastError::config(
            "auto-list without --dry-run needs a Vast.ai API key; run it with \
             --dry-run to see the decisions, or `stado market readiness --provider vast` \
             for what is missing",
        ));
    }
    // WC_VAST_MAX_DURATION_S env wins (cli.py uneditable).
    let mut duration_s = params.duration_s;
    if let Some(raw) =
        crate::config_file::variable("WC_VAST_MAX_DURATION_S").map_err(VastError::config)?
    {
        if !raw.is_empty() {
            duration_s = Some(raw.trim().parse::<i64>().map_err(|error| {
                VastError::config(format!(
                    "WC_VAST_MAX_DURATION_S must be int: {raw}: {error}"
                ))
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
                let cur_price = status.get("listed_gpu_cost");
                // An unlisted machine answers no price, or a zero one.
                if let Some(current) = cur_price
                    .and_then(Value::as_f64)
                    .filter(|price| price.is_normal() && price.is_sign_positive())
                {
                    listed = true;
                    log(&format!(
                        "startup: listed at ${}/h on machine_id={}",
                        py_float(current),
                        shown(status.get("id"))
                    ));
                    // Any listed price other than the configured one is
                    // normalized; the configured price is what Vast is sent.
                    if current != params.price_gpu && !params.dry_run {
                        let normalize = async {
                            client.unlist_machine().await?;
                            client
                                .list_machine(&ListMachineParams {
                                    price_gpu: params.price_gpu,
                                    price_disk: params.price_disk,
                                    price_min_bid: None,
                                    duration: duration_s,
                                })
                                .await
                        };
                        match normalize.await {
                            Ok(_) => log(&format!(
                                "normalized ${}/h -> ${}/h",
                                py_float(current),
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
    // One poll at once, then one every `--poll-interval-s`; a single poll
    // (`--once`) returns before the second.
    let mut pace = params.poll.map(|poll| {
        let mut pace = tokio::time::interval(poll);
        pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        pace
    });
    loop {
        if let Some(pace) = pace.as_mut() {
            pace.tick().await;
        }
        // A reservation record that cannot be read may be one, so it holds the
        // cards exactly as a readable one does, and every line names its cause.
        let held = match crate::inference::reservation::active() {
            Ok(Some(reservation)) => Some(format!(
                "inference reservation '{}'",
                reservation.deployment
            )),
            Ok(None) => None,
            Err(error) => Some(format!(
                "an inference reservation that cannot be read ({error})"
            )),
        };
        if let Some(held) = held {
            idle_since = None;
            if listed {
                if params.dry_run {
                    log(&format!("DRY-RUN would unlist for {held}"));
                } else if let Some(client) = client {
                    match client.unlist_machine().await {
                        Ok(_) => {
                            listed = false;
                            log(&format!("unlisted for {held}"));
                        }
                        Err(exc) => log(&format!("unlist for {held} failed: {exc}")),
                    }
                }
            }
            if params.once {
                return Ok(());
            }
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
            match decide_action(listed, &state, Some(idle_dur), params.idle_window_s) {
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
                                price_disk: params.price_disk,
                                price_min_bid: None,
                                duration: duration_s,
                            })
                            .await
                        {
                            Ok(_) => {
                                listed = true;
                                let duration = match duration_s {
                                    Some(seconds) => format!("{seconds}s"),
                                    None => "open-ended".to_string(),
                                };
                                log(&format!(
                                    "LISTED ({idle_dur_s}s idle, ${}/h, dur={duration})",
                                    py_float(params.price_gpu),
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
            let decided = decide_action(listed, &state, None, params.idle_window_s);
            if decided == AutoListAction::Unlist {
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
            // less free VRAM than that job declares, so something is holding
            // the GPU and the claim loop will sit idle until it lets go. The
            // line says what was read and marks the rental as the inference
            // it is: on a machine with no Vast credential at all it said
            // "waiting for Vast rental to finish" over a queue of two jobs
            // and a laptop GPU, which is a state that could not exist.
            match decided {
                AutoListAction::WaitingForRental {
                    free_vram_gb,
                    need_gb,
                } if client.is_some() => log(&format!(
                    "not listed, queued={}, free_vram_gb={} below the {need_gb} GB the queued \
                         job needs: something holds the GPU, most likely a Vast rental; \
                         wisent-compute claims when it releases",
                    state.queued,
                    py_float(free_vram_gb)
                )),
                AutoListAction::WaitingForRental {
                    free_vram_gb,
                    need_gb,
                } => log(&format!(
                    "not listed, queued={}, free_vram_gb={} below the {need_gb} GB the queued \
                     job needs: the GPU is occupied and this host holds no Vast credential, so \
                     no rental of ours is on it",
                    state.queued,
                    py_float(free_vram_gb)
                )),
                AutoListAction::BusyNotListed => log(&format!("busy ({}); not listed", counts())),
                AutoListAction::Unlist
                | AutoListAction::List { .. }
                | AutoListAction::IdleCountdown { .. } => {}
            }
        }
        if params.once {
            return Ok(());
        }
    }
}
