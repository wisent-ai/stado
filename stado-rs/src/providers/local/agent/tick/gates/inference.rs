//! Release drift, the Vast renter, and the inference reservation that may be
//! holding this host's GPU — everything that decides how much VRAM the claim
//! scan is allowed to see.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::providers::local::agent::capacity::inference::{
    inference_container_running, queued_gpu_job_for_inference, set_inference_container_running,
};
use crate::providers::local::agent::capacity::snapshot::{
    diag_map, measured_capacity, publish_branch,
};
use crate::providers::local::agent::Step;
use crate::providers::local::disk::gate;
use crate::providers::local::helpers;
use crate::providers::local::probe::version_check::{self, DriftOutcome};
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

/// Report `(free VRAM, per-card frees)` for the rest of this tick, after the
/// release check and the inference reservation have both had their say.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn before_admission(
    store: &JobStorage,
    sizing: &Sizing,
    consumer_id: &str,
    kind: &str,
    gpu_type: &str,
    total_vram_gb: i64,
    pinned_only: bool,
    vast_active: bool,
    slots: &[ActiveSlot],
    agent_diag: &mut Map<String, Value>,
    last_cap: &mut Option<CapacitySnapshot>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<(i64, Vec<helpers::GpuCard>)>> {
    // Cleanup already ran before the immutable release check. This gate is
    // admission/diagnostics-only and has no destructive side effects.
    let (_pre_refuse, pre_diag) = gate::measure_admission(log_fn);
    agent_diag.extend(diag_map(&pre_diag));
    log_fn("loop: pre-drain release drift check");
    match version_check::maybe_drain_or_upgrade(!slots.is_empty(), log_fn, kind).await {
        // An update/re-exec failure was logged; keep claiming on the old
        // binary rather than wedging the fleet. A successful update
        // re-execs and never reaches this arm.
        DriftOutcome::Clean | DriftOutcome::DriftDetected => {}
        // Cloud replacement was requested through the provider adapter.
        DriftOutcome::SelfTerminated => return Ok(Step::Stop),
    }
    // A reservation record that cannot be read may be an exclusive one: GPU
    // claims are disabled below and the cause is published, rather than the
    // cards being offered as if no engine held them.
    let (inference_reservation, reservation_error) = match crate::inference::reservation::active() {
        Ok(reservation) => (reservation, None),
        Err(error) => (None, Some(error)),
    };
    if vast_active {
        let snapshot = measured_capacity(
            slots,
            false,
            Some("vast_renter_active"),
            BTreeMap::new(),
            None,
            total_vram_gb,
            agent_diag.clone(),
        );
        publish_branch(
            store,
            consumer_id,
            kind,
            "vast-renter-active",
            &snapshot,
            log_fn,
        )
        .await?;
        *last_cap = Some(snapshot);
        return Ok(Step::Done);
    }

    let mut slot_vram = Vec::new();
    for s in slots {
        slot_vram.push(helpers::slot_vram(&s.slot, sizing, store).await?);
    }
    let mut used_vram: i64 = slot_vram.iter().sum();
    if slots.iter().any(|s| helpers::slot_is_exclusive(&s.slot)) {
        used_vram = total_vram_gb;
    }
    if let Some(error) = &reservation_error {
        // Held as an exclusive reservation holds them: no GPU claim fits, CPU
        // claims stay eligible, and the cause is published with the capacity.
        used_vram = total_vram_gb;
        agent_diag.insert(
            "inference_reservation_error".into(),
            Value::from(error.clone()),
        );
        log_fn(&format!(
            "inference reservation cannot be read; GPU claims disabled, CPU-only claims remain eligible: {error}"
        ));
    }
    // Every card, one driver read. `free_vram_gb` stays the answer to "will
    // one job fit", which is the emptiest board; the per-card list carries
    // the rest of the truth into the broadcast and into device selection,
    // because a host with two boards has two independent pools and the
    // pooled reading was wrong about both. A driver that cannot be read says
    // nothing about what the cards hold; it was read as no cards, and the
    // bookkeeping figure then admitted GPU work onto boards an outside
    // process may fill. GPU claims are held, as an unreadable reservation
    // holds them, and the cause is published.
    let mut cards = match helpers::smi_gpu_cards().await {
        Ok(cards) => {
            agent_diag.remove("gpu_driver_error");
            cards
        }
        Err(cause) => {
            used_vram = total_vram_gb;
            agent_diag.insert("gpu_driver_error".into(), Value::from(cause.clone()));
            log_fn(&format!(
                "the GPU driver cannot be read; GPU claims disabled, CPU-only claims remain eligible: {cause}"
            ));
            Vec::new()
        }
    };
    cards.sort_by_key(|card| std::cmp::Reverse(card.free_vram_gb));
    // Who holds the cards, published beside the free/total figures so a
    // `0/95 GiB free` line can name the process behind it.
    crate::providers::local::accelerators::holders::measure(slots)
        .await
        .insert_into(
            agent_diag,
            total_vram_gb,
            free_for_one_job(total_vram_gb, used_vram, &cards),
        );
    if let Some(reservation) = &inference_reservation {
        agent_diag.insert(
            "inference_reservation".into(),
            Value::from(reservation.deployment.clone()),
        );
        agent_diag.insert(
            "inference_gpu_mode".into(),
            Value::String(reservation.gpu_mode.to_string()),
        );
        if reservation.gpu_mode == crate::inference::schema::GpuMode::Exclusive {
            match reservation.gpu.as_deref() {
                // A reservation that names its board withdraws that board
                // alone: the driver's other boards stay admissible, so one
                // engine no longer empties a host of every board it has.
                Some(board) if cards.iter().any(|card| card.uuid == board) => {
                    cards.retain(|card| card.uuid != board);
                    agent_diag.insert("inference_gpu".into(), Value::from(board));
                    log_fn(&format!(
                        "exclusive inference reservation '{}' holds board {board}; GPU claims \
                         stay eligible on the host's other {} board(s)",
                        reservation.deployment,
                        cards.len()
                    ));
                    if cards.is_empty() {
                        used_vram = total_vram_gb;
                    }
                }
                // A named board the driver does not report cannot be told
                // apart from the others, so every board stays held, and why
                // is published.
                Some(board) => {
                    used_vram = total_vram_gb;
                    agent_diag.insert(
                        "inference_gpu_error".into(),
                        Value::from(format!(
                            "the reservation names board {board}, which the driver does not \
                             report; every board stays held"
                        )),
                    );
                    log_fn(&format!(
                        "exclusive inference reservation '{}' names board {board}, which the \
                         driver does not report: GPU claims disabled",
                        reservation.deployment
                    ));
                }
                None => {
                    used_vram = total_vram_gb;
                    log_fn(&format!(
                        "exclusive inference reservation '{}' names no board: GPU claims \
                         disabled; CPU-only claims remain eligible",
                        reservation.deployment
                    ));
                }
            }
        } else {
            let queued_job = queued_gpu_job_for_inference(
                store,
                sizing,
                gpu_type,
                total_vram_gb,
                kind,
                consumer_id,
                slots.len(),
                pinned_only,
            )
            .await?;
            let gpu_work_active = used_vram.is_positive();
            // A queued job takes the board only when it is worth more per
            // board-hour than what the deployment earns there; an admitted
            // job already running keeps it until it ends.
            let queued_wins = match &queued_job {
                Some((job, _)) => {
                    let worker = crate::config::worker_name();
                    let decision =
                        weigh(store, &worker, &reservation.deployment, job, log_fn).await;
                    agent_diag.insert(
                        "board_decision".into(),
                        serde_json::to_value(&decision).unwrap_or(Value::Null),
                    );
                    decision.chosen
                        == Some(super::super::super::allocate::Use::Job {
                            job_id: job.job_id.clone(),
                        })
                }
                None => false,
            };
            let should_yield = gpu_work_active || queued_wins;
            match inference_container_running(&reservation.deployment).await {
                Ok(true) if should_yield => {
                    let reason = match &queued_job {
                        Some((job, need)) if queued_wins => {
                            format!(
                                "queued job {} needs {need} GiB and is worth more",
                                job.job_id
                            )
                        }
                        _ => "an admitted GPU job is active".to_string(),
                    };
                    log_fn(&format!(
                        "yieldable inference '{}': pausing because {reason}",
                        reservation.deployment
                    ));
                    match set_inference_container_running(&reservation.deployment, false).await {
                        Ok(()) => return Ok(Step::Done),
                        Err(error) => {
                            log_fn(&format!(
                                "yieldable inference '{}': pause failed safely: {error}",
                                reservation.deployment
                            ));
                            used_vram = total_vram_gb;
                        }
                    }
                }
                Ok(true) => {
                    used_vram = total_vram_gb;
                    agent_diag.insert("inference_runtime_state".into(), Value::from("serving"));
                }
                Ok(false) if !should_yield && slots.is_empty() => {
                    agent_diag.insert("inference_runtime_state".into(), Value::from("resuming"));
                    let snapshot = measured_capacity(
                        slots,
                        false,
                        Some("inference_resuming"),
                        BTreeMap::new(),
                        None,
                        total_vram_gb,
                        agent_diag.clone(),
                    );
                    publish_branch(
                        store,
                        consumer_id,
                        kind,
                        "inference-resuming",
                        &snapshot,
                        log_fn,
                    )
                    .await?;
                    *last_cap = Some(snapshot);
                    log_fn(&format!(
                        "yieldable inference '{}': GPU queue drained; resuming service",
                        reservation.deployment
                    ));
                    if let Err(error) =
                        set_inference_container_running(&reservation.deployment, true).await
                    {
                        log_fn(&format!(
                            "yieldable inference '{}': resume failed: {error}",
                            reservation.deployment
                        ));
                    }
                    return Ok(Step::Done);
                }
                Ok(false) => {
                    agent_diag.insert("inference_runtime_state".into(), Value::from("yielded"));
                    if let Some((job, need)) = queued_job.filter(|_| queued_wins) {
                        agent_diag
                            .insert("inference_yield_for_job".into(), Value::from(job.job_id));
                        agent_diag.insert("inference_yield_for_vram_gb".into(), Value::from(need));
                    }
                }
                Err(error) => {
                    used_vram = total_vram_gb;
                    agent_diag.insert("inference_runtime_state".into(), Value::from("unknown"));
                    log_fn(&format!(
                        "yieldable inference '{}': runtime state unavailable; GPU claims disabled: {error}",
                        reservation.deployment
                    ));
                }
            }
        }
    }
    Ok(Step::Go((
        free_for_one_job(total_vram_gb, used_vram, &cards),
        cards,
    )))
}

/// The board's decision between the queued `job` and the Yieldable
/// `deployment` holding it, each valued per board-hour, kept with its inputs.
/// The job's value is the income its submitter stated, else the autonomy
/// policy's `budgets.internal_usd_per_gpu_hour`; the deployment's is what
/// outside callers paid for it over the policy's
/// `freshness.pricing_max_age_seconds`, the age the operator lets a price
/// be. A side whose value cannot be read is a refused candidate, named,
/// never valued at zero.
async fn weigh(
    store: &JobStorage,
    host: &str,
    deployment: &str,
    job: &crate::models::Job,
    log_fn: &mut dyn FnMut(&str),
) -> super::super::super::allocate::Decision {
    use super::super::super::allocate::{self, Candidate, Use};
    let policy = match crate::autonomy::storage::load_policy(store).await {
        Ok(Some(policy)) => Ok(policy),
        Ok(None) => Err("no autonomy policy is written".to_string()),
        Err(error) => Err(format!("the autonomy policy cannot be read: {error}")),
    };
    let job_use = Use::Job {
        job_id: job.job_id.clone(),
    };
    let job_candidate = match (job.income_usd_per_hour, &job.income_source, &policy) {
        (Some(income), Some(source), _) => {
            Candidate::valued(job_use, income, format!("stated by {source}"))
        }
        (_, _, Ok(policy)) => match policy.budgets.internal_usd_per_gpu_hour {
            Some(value) => Candidate::valued(
                job_use,
                value,
                "internal work at the policy's budgets.internal_usd_per_gpu_hour",
            ),
            None => Candidate::refused(
                job_use,
                "the job states no income and the autonomy policy declares no \
                 budgets.internal_usd_per_gpu_hour",
            ),
        },
        (_, _, Err(cause)) => {
            Candidate::refused(job_use, format!("the job states no income and {cause}"))
        }
    };
    let deployment_use = Use::Deployment {
        name: deployment.to_string(),
    };
    let deployment_candidate = match &policy {
        Err(cause) => Candidate::refused(
            deployment_use.clone(),
            format!("its earnings window is the policy's freshness.pricing_max_age_seconds, and {cause}"),
        ),
        Ok(policy) => {
            let seconds = policy.freshness.pricing_max_age_seconds;
            let since = i64::try_from(seconds)
                .ok()
                .and_then(chrono::Duration::try_seconds)
                .and_then(|window| chrono::Utc::now().checked_sub_signed(window));
            match since {
                None => Candidate::refused(
                    deployment_use.clone(),
                    format!("freshness.pricing_max_age_seconds {seconds} reaches before any time"),
                ),
                Some(since) => match allocate::deployment_value(store, deployment, since).await {
                Ok(Ok(value)) => Candidate::valued(
                    deployment_use.clone(),
                    value,
                    format!("what outside callers paid over the last {seconds} s"),
                ),
                Ok(Err(cause)) => Candidate::refused(deployment_use.clone(), cause),
                Err(error) => Candidate::refused(
                    deployment_use.clone(),
                    format!("its callers' usage cannot be read: {error}"),
                ),
                },
            }
        }
    };
    let decision = allocate::decide(
        host,
        deployment,
        Some(deployment_use),
        vec![job_candidate, deployment_candidate],
        None,
    );
    log_fn(&format!("board of '{deployment}': {}", decision.reason));
    if let Err(error) = allocate::keep(store, &decision).await {
        log_fn(&format!(
            "board of '{deployment}': the decision is not kept: {error}"
        ));
    }
    decision
}

/// The free VRAM one job can use: what the bookkeeping leaves of the total
/// (a hold counts as the whole total used), and no more than the emptiest
/// board the driver reports when it reports one.
fn free_for_one_job(total_vram_gb: i64, used_vram: i64, cards: &[helpers::GpuCard]) -> i64 {
    let booked = total_vram_gb - used_vram.min(total_vram_gb);
    match cards
        .iter()
        .map(|card| card.free_vram_gb)
        .filter(|free| !free.is_negative())
        .max()
    {
        Some(emptiest) => booked.min(emptiest),
        None => booked,
    }
}
