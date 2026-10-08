//! What this host may claim right now: the bounded claimable-job listing and
//! the one exception disk pressure still admits.

use serde_json::{Map, Value};

use crate::models::Job;
use crate::providers::local::agent::Step;
use crate::providers::local::helpers;
use crate::providers::local::slots::{job_system_packages_eligible, ActiveSlot};
use crate::queue::JobStorage;

/// The one job a host under disk pressure still runs: a signed Stado release
/// delivery pinned to it, which is how a release reaches the agent that owns
/// the pressure rule. It is also the one job that starts while a janitor pass
/// holds the workload lock ([`super::start`]).
pub(crate) fn is_signed_stado_delivery(job: &Job) -> bool {
    job.gpu_mem_gb == 0
        && job.priority == crate::primitives::constants::RELEASE_JOB_PRIORITY
        && !job.run_id.is_empty()
        && !job.pinned_host.is_empty()
        && job.command == crate::primitives::constants::RELEASE_DELIVERY_JOB_COMMAND
        && job
            .output_uri
            .starts_with("stado://probierz/runs/release-pipeline/stado/")
        && job.output_uri.contains("/deliveries/")
        && job.output_uri.ends_with("/output")
}

/// A job that stages nothing on this host's disk: a command pinned to this
/// host that clones no repository, installs no packages, runs no pre-command
/// and mirrors no output. Disk pressure blocks work because work fills the
/// disk; a pinned in-place command (an Oko routine reading this host's own
/// transcripts and terminals) adds nothing the janitor could reclaim, so
/// refusing it only stops the host's upkeep — forever on a host whose disk
/// the user's own data keeps above the rule (d8c28fc2).
pub(crate) fn stages_nothing(job: &Job) -> bool {
    job.gpu_mem_gb == 0
        && !job.pinned_host.is_empty()
        && job.repo.is_empty()
        && job.apt_packages.is_empty()
        && job.pre_command.is_empty()
        && job.output_uri.is_empty()
}

/// Read the fresh queue documents this tick may admit, newest operator intent
/// first while the host is under its disk watermark.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn claimable(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    gpu_type: &str,
    total_vram_gb: i64,
    free_vram_gb: i64,
    pinned_only: bool,
    pressure_active: bool,
    current_free_bytes: Option<i64>,
    disk_low_bytes: Option<i64>,
    slots: &[ActiveSlot],
    agent_diag: &mut Map<String, Value>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<Vec<Job>>> {
    // Centralized assignment writes job.assigned_to on the queue blob, so
    // the listing itself applies this agent's full admission rule: the
    // window must count jobs this host may claim. Counting jobs that
    // merely fit its VRAM meant a fleet whose oldest two thousand fitting
    // jobs were assigned elsewhere handed this agent nothing claimable on
    // every poll, forever, while its own assigned job sat past the window.
    // The re-read below re-applies the rule to the FRESH document, which
    // is a different fact from the listed snapshot. The capacity heartbeat
    // keeps speaking for the host while this read runs.
    let listed = store
        .list_claimable_jobs(
            "queue",
            &crate::queue::listing::JobScan {
                want: 0,
                scan_budget: 0,
                max_gpu_mem_gb: free_vram_gb,
                eligible: &|job| {
                    helpers::job_eligible(
                        job,
                        gpu_type,
                        total_vram_gb,
                        kind,
                        consumer_id,
                        slots.len(),
                        pinned_only,
                    ) && job_system_packages_eligible(job, kind)
                },
                // A claim loop wants reachability and so takes the
                // shared rotation: a job past this poll's window is
                // reached by a later poll rather than never.
                from_head: false,
            },
        )
        .await?;
    let mut queued = Vec::with_capacity(listed.len());
    for candidate in listed {
        if let Some(job) = store.read_job("queue", &candidate.job_id).await? {
            queued.push(job);
        }
    }
    queued.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    if pressure_active {
        // A host can accumulate deliveries while it is under pressure.
        // The newest submission is the current operator intent; replaying
        // them FIFO briefly downgrades the installed agent before climbing
        // through every superseded coordinate.
        let (mut deliveries, others): (Vec<Job>, Vec<Job>) =
            queued.into_iter().partition(is_signed_stado_delivery);
        let matched_deliveries = deliveries.len();
        deliveries.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.job_id.cmp(&a.job_id))
        });
        deliveries.truncate(1);
        let in_place: Vec<Job> = others.into_iter().filter(stages_nothing).collect();
        agent_diag.insert(
            "disk_pressure_superseded_deliveries".into(),
            Value::from(matched_deliveries.saturating_sub(deliveries.len()) as i64),
        );
        agent_diag.insert(
            "disk_pressure_recovery_jobs".into(),
            Value::from(deliveries.len() as i64),
        );
        agent_diag.insert(
            "disk_pressure_in_place_jobs".into(),
            Value::from(in_place.len() as i64),
        );
        let (delivery_count, in_place_count) = (deliveries.len(), in_place.len());
        queued = deliveries.into_iter().chain(in_place).collect();
        if queued.is_empty() {
            log_fn(&format!(
                "loop: disk-pressure-active: free bytes {current_free_bytes:?} are under the \
                 {disk_low_bytes:?} byte low watermark; ordinary work remains blocked and \
                 neither a signed Stado release delivery nor a pinned job that stages nothing \
                 is assigned to this host"
            ));
            return Ok(Step::Done);
        }
        log_fn(&format!(
            "loop: disk-pressure-active: free bytes {current_free_bytes:?} are under the \
             {disk_low_bytes:?} byte low watermark; admitting {delivery_count} signed Stado \
             release delivery, {in_place_count} pinned job(s) that stage nothing on this disk, \
             and no ordinary work"
        ));
    }
    Ok(Step::Go(queued))
}
