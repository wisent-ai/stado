//! One offer, priced and constraint-checked: what it would cost and every
//! reason it cannot be used.
//!
//! [`candidate`] prices the run — startup, compute, retry risk, egress and
//! the SLO penalty a deadline earns — and then collects every reason the
//! job, the policy and the machine shape give for refusing this offer. A
//! candidate with an empty reason list and a total cost is the only kind the
//! pass may select.

use std::collections::BTreeSet;

use chrono::Utc;

use crate::autonomy::cost::PriceQuote;
use crate::autonomy::optimizer::types::{CandidateContext, CapacityOffer, PlacementCandidate};
use crate::capabilities::ProviderId;
use crate::models::Job;

use super::feedback::{observed_failure_probability, observed_startup_seconds};
use super::shapes::{crosses_provider_boundary, machine_capacity};

const CLOUD_STARTUP_SECONDS: f64 =
    (crate::monitor::billing::SECONDS_PER_MINUTE * (u16::BITS / u8::BITS) as u64) as f64;
const DEFAULT_FAILURE_PROBABILITY: f64 = f64::EPSILON;

pub(super) fn candidate(
    job: &Job,
    offer: &CapacityOffer,
    runtime: f64,
    preemptible: bool,
    quote: Option<&PriceQuote>,
    possible_regions: &BTreeSet<String>,
    context: &CandidateContext<'_>,
) -> PlacementCandidate {
    let policy = context.policy;
    let feedback = context.feedback;
    let new_cloud_hourly_budget_usd = context.budget.hourly_usd;
    let new_cloud_cost_budget_usd = context.budget.total_usd;
    let startup = if offer.existing {
        f64::default()
    } else {
        observed_startup_seconds(feedback, &offer.target_id).unwrap_or(CLOUD_STARTUP_SECONDS)
    };
    let failure_probability = observed_failure_probability(feedback, &offer.target_id)
        .unwrap_or(DEFAULT_FAILURE_PROBABILITY);
    let hourly = if offer.provider == ProviderId::Local && quote.is_none() {
        policy.local_hourly_cost_usd.or(Some(f64::default()))
    } else {
        quote.map(|price| price.hourly_usd)
    };
    let expected_finish = runtime + startup;
    let mut deadline_rejection = None;
    let slo_penalty = job
        .deadline_at
        .as_deref()
        .map(|raw| match chrono::DateTime::parse_from_rfc3339(raw) {
            Ok(deadline) => {
                let remaining = (deadline.with_timezone(&Utc) - context.now).num_milliseconds()
                    as f64
                    / chrono::Duration::seconds(true as i64).num_milliseconds() as f64;
                let lateness = (expected_finish - remaining).max(f64::default());
                if lateness > f64::default() {
                    deadline_rejection = Some(format!(
                        "completion deadline would be missed by {lateness} seconds"
                    ));
                }
                hourly.unwrap_or_default() * lateness
                    / crate::monitor::billing::SECONDS_PER_HOUR as f64
            }
            Err(error) => {
                deadline_rejection = Some(format!("invalid completion deadline: {error}"));
                f64::default()
            }
        })
        .unwrap_or_default();
    let compute = hourly
        .map(|rate| rate * (runtime + startup) / crate::monitor::billing::SECONDS_PER_HOUR as f64);
    let retry = compute.map(|cost| cost * failure_probability);
    let egress =
        if policy.placement.account_for_egress && crosses_provider_boundary(job, offer.provider) {
            None
        } else {
            Some(f64::default())
        };
    let total = compute
        .zip(egress)
        .zip(retry)
        .map(|((compute, egress), retry)| compute + egress + retry + slo_penalty);
    let mut rejected = Vec::new();
    if let Some(reason) = deadline_rejection {
        rejected.push(reason);
    }
    if offer.available_instances <= i64::default() {
        rejected.push("no available instances".to_string());
    }
    if offer.free_vram_gb > i64::default() && offer.free_vram_gb < job.gpu_mem_gb {
        rejected.push(format!(
            "free VRAM {} GiB is below required {} GiB",
            offer.free_vram_gb, job.gpu_mem_gb
        ));
    }
    if job.pin_to_provider && !job.provider.is_empty() && job.provider != offer.provider.as_str() {
        rejected.push(format!("job is pinned to provider {}", job.provider));
    }
    if !job.pinned_host.is_empty() && !offer.target_id.eq_ignore_ascii_case(&job.pinned_host) {
        rejected.push(format!("job is pinned to host {}", job.pinned_host));
    }
    if !offer.existing
        && matches!(
            offer.provider,
            ProviderId::Gcp | ProviderId::Aws | ProviderId::Azure
        )
        && (new_cloud_hourly_budget_usd
            .is_some_and(|remaining| hourly.is_none_or(|rate| rate > remaining))
            || new_cloud_cost_budget_usd
                .is_some_and(|remaining| total.is_none_or(|cost| cost > remaining)))
    {
        rejected.push("budget guard blocks new cloud capacity".to_string());
    }
    if !job.gpu_type.is_empty() && job.gpu_type != offer.accelerator_type {
        rejected.push(format!("accelerator must be {}", job.gpu_type));
    }
    if !job.machine_type.is_empty()
        && !offer.machine_type.is_empty()
        && job.machine_type != offer.machine_type
    {
        rejected.push(format!("machine type must be {}", job.machine_type));
    }
    if !job.region.is_empty() && offer.region.as_deref() != Some(job.region.as_str()) {
        rejected.push(format!("region must be {}", job.region));
    }
    if !offer.existing
        && !job.region.is_empty()
        && possible_regions.iter().any(|region| region != &job.region)
    {
        rejected.push(format!(
            "provider substitution could leave required region {}",
            job.region
        ));
    }
    if job.cpu_cores > i64::default() || job.memory_gb > i64::default() {
        match machine_capacity(offer.provider, &offer.machine_type) {
            Some((cpu, memory)) if cpu >= job.cpu_cores && memory >= job.memory_gb => {}
            Some((cpu, memory)) => rejected.push(format!(
                "shape has {cpu} CPU/{memory} GiB but job needs {}/{}",
                job.cpu_cores, job.memory_gb
            )),
            None => rejected.push(
                "provider shape capacity is unknown for explicit CPU/RAM constraints".to_string(),
            ),
        }
    }
    if !job.platform_os.is_empty()
        && offer.provider != ProviderId::Local
        && !job.platform_os.eq_ignore_ascii_case("linux")
    {
        rejected.push(format!("platform {} is unavailable", job.platform_os));
    }
    if !job.architecture.is_empty()
        && offer.provider != ProviderId::Local
        && !matches!(job.architecture.as_str(), "amd64" | "x86_64")
    {
        rejected.push(format!("architecture {} is unavailable", job.architecture));
    }
    if !policy.placement.allowed_regions.is_empty() {
        let offered_region_forbidden = offer
            .region
            .as_ref()
            .is_none_or(|region| !policy.placement.allowed_regions.contains(region));
        let alternate_region_forbidden = !offer.existing
            && possible_regions
                .iter()
                .any(|region| !policy.placement.allowed_regions.contains(region));
        if offered_region_forbidden || alternate_region_forbidden {
            rejected.push("region is not allowed by policy".to_string());
        }
    }
    if quote.is_none() && offer.provider != ProviderId::Local {
        rejected.push("no fresh dynamic hourly price".to_string());
    }
    if egress.is_none() {
        rejected.push("cross-provider data egress cannot be priced from job metadata".to_string());
    }
    if job.max_cost_per_hour_usd > f64::default()
        && hourly.is_some_and(|rate| rate > job.max_cost_per_hour_usd)
    {
        rejected.push(format!(
            "hourly rate exceeds job cap {:.6}",
            job.max_cost_per_hour_usd
        ));
    }
    if let Some(limit) = policy.budgets.hourly_usd {
        if hourly.is_some_and(|rate| rate > limit) {
            rejected.push(format!("hourly rate exceeds autonomy budget {limit}"));
        }
    }
    if let Some(limit) = policy.budgets.max_single_action_usd {
        if total.is_some_and(|cost| cost > limit) {
            rejected.push(format!("expected job cost exceeds action budget {limit}"));
        }
    }
    PlacementCandidate {
        target_id: offer.target_id.clone(),
        provider: offer.provider,
        region: offer.region.clone(),
        machine_type: offer.machine_type.clone(),
        accelerator_type: offer.accelerator_type.clone(),
        vram_gb: offer.free_vram_gb,
        available_instances: offer.available_instances,
        existing_capacity: offer.existing,
        preemptible,
        startup_seconds: startup,
        runtime_seconds: runtime,
        hourly_compute_usd: hourly,
        compute_cost_usd: compute,
        storage_cost_usd: f64::default(),
        egress_cost_usd: egress,
        retry_risk_cost_usd: retry,
        slo_penalty_usd: slo_penalty,
        expected_total_cost_usd: total,
        expected_finish_seconds: expected_finish,
        price_source: quote.map(|price| price.source.clone()),
        eligible: rejected.is_empty() && total.is_some(),
        rejected_reasons: rejected,
    }
}
