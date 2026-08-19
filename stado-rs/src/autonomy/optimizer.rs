//! Global, atomic workload placement across local capacity and cloud providers.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::cost::{PriceBook, PriceQuote};
use super::model::{DecisionKind, DecisionRecord, SavingsRecord, SCHEMA_VERSION};
use super::policy::AutonomyPolicy;
use crate::capabilities::ProviderId;
use crate::models::Job;
use crate::providers::Provider;
use crate::queue::{capacity, JobStorage, StorageError};

const TWO: f64 = (u16::BITS / u8::BITS) as f64;
const CLOUD_STARTUP_SECONDS: f64 =
    (crate::monitor::billing::SECONDS_PER_MINUTE * (u16::BITS / u8::BITS) as u64) as f64;
const DEFAULT_FAILURE_PROBABILITY: f64 = f64::EPSILON;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementCandidate {
    pub target_id: String,
    pub provider: ProviderId,
    pub region: Option<String>,
    pub machine_type: String,
    pub accelerator_type: String,
    pub vram_gb: i64,
    pub available_slots: i64,
    pub existing_capacity: bool,
    pub preemptible: bool,
    pub startup_seconds: f64,
    pub runtime_seconds: f64,
    pub hourly_compute_usd: Option<f64>,
    pub compute_cost_usd: Option<f64>,
    pub storage_cost_usd: f64,
    pub egress_cost_usd: Option<f64>,
    pub retry_risk_cost_usd: Option<f64>,
    pub slo_penalty_usd: f64,
    pub expected_total_cost_usd: Option<f64>,
    pub expected_finish_seconds: f64,
    pub price_source: Option<String>,
    pub eligible: bool,
    pub rejected_reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlacementRunSummary {
    pub considered_jobs: usize,
    pub decided_jobs: usize,
    pub changed_jobs: usize,
    pub no_eligible_target: usize,
    pub active_lease_skips: usize,
    pub provider_errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct CapacityOffer {
    target_id: String,
    provider: ProviderId,
    region: Option<String>,
    accelerator_type: String,
    machine_type: String,
    free_vram_gb: i64,
    free_slots: i64,
    existing: bool,
}

#[derive(Debug, Clone, Copy)]
struct CloudBudget {
    hourly_usd: Option<f64>,
    total_usd: Option<f64>,
}

struct CandidateContext<'a> {
    policy: &'a AutonomyPolicy,
    feedback: &'a [super::storage::PlacementFeedback],
    budget: CloudBudget,
    now: chrono::DateTime<Utc>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "public coordinator API keeps the independent hourly and total budget controls explicit"
)]
pub async fn plan_queued(
    store: &JobStorage,
    cloud_providers: &[(String, Arc<dyn Provider>)],
    policy: &AutonomyPolicy,
    prices: &PriceBook,
    inventory_snapshot_id: &str,
    log: &dyn Fn(&str),
    mut new_cloud_hourly_budget_usd: Option<f64>,
    mut new_cloud_cost_budget_usd: Option<f64>,
) -> Result<PlacementRunSummary, StorageError> {
    let mut summary = PlacementRunSummary::default();
    if policy.mode == super::policy::AutonomyMode::Report {
        return Ok(summary);
    }
    if policy.emergency_paused {
        log("optimizer: emergency pause active; no placement mutations");
        return Ok(summary);
    }
    let (mut offers, provider_errors) = collect_offers(store, cloud_providers, policy).await?;
    summary.provider_errors = provider_errors;
    let history_rows = crate::scheduler::cost::collect_completed(store).await?;
    let wall_times = crate::scheduler::cost::wall_time_table(&history_rows);
    let feedback = super::storage::list_feedback(store).await?;
    let planning_now = Utc::now();
    let mut queued = store
        .list_jobs_priority_first("queue", usize::default())
        .await?;
    queued.sort_by(job_order);
    for job in queued {
        summary.considered_jobs += true as usize;
        if !job.pinned_host.is_empty() && job.pinned_host != job.assigned_to {
            continue;
        }
        let candidate_context = CandidateContext {
            policy,
            feedback: &feedback,
            budget: CloudBudget {
                hourly_usd: new_cloud_hourly_budget_usd,
                total_usd: new_cloud_cost_budget_usd,
            },
            now: planning_now,
        };
        let candidates = candidates_for_job(&job, &offers, prices, &wall_times, &candidate_context);
        let selected = candidates
            .iter()
            .filter(|candidate| candidate.eligible)
            .min_by(|left, right| candidate_order(left, right));
        let Some(selected) = selected else {
            summary.no_eligible_target += true as usize;
            persist_unplaced_decision(store, &job, candidates, policy, inventory_snapshot_id)
                .await?;
            continue;
        };
        let decision_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let lease = super::storage::acquire_placement_lease(
            store,
            &job.job_id,
            &decision_id,
            "coordinator-placement",
            policy.limits.decision_ttl_seconds,
            now,
        )
        .await?;
        let Some(lease) = lease else {
            summary.active_lease_skips += true as usize;
            continue;
        };
        let explanation = explain_selection(selected, &candidates);
        let decision = DecisionRecord {
            schema_version: SCHEMA_VERSION,
            decision_id: decision_id.clone(),
            kind: DecisionKind::Placement,
            subject_id: job.job_id.clone(),
            created_at: now.to_rfc3339(),
            expires_at: lease.expires_at.clone(),
            inventory_snapshot_id: inventory_snapshot_id.to_string(),
            policy_version: policy.policy_version.clone(),
            selected: Some(serde_json::to_value(selected)?),
            candidates: candidates
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?,
            constraints: placement_constraints(&job, policy),
            explanation,
            lease_token: Some(lease.token.clone()),
            state: "leased".to_string(),
        };
        if let Err(error) = super::storage::write_decision(store, &decision).await {
            let _ = super::storage::release_placement_lease(store, &job.job_id, &lease.token).await;
            return Err(error);
        }
        summary.decided_jobs += true as usize;
        match update_job_placement(store, &job, selected).await {
            Ok(true) => {
                summary.changed_jobs += true as usize;
                if let Err(error) =
                    persist_predicted_savings(store, &decision_id, &job, selected, &candidates, now)
                        .await
                {
                    log(&format!(
                        "optimizer: savings record for {} failed: {error}",
                        job.job_id
                    ));
                }
                reserve_offer(&mut offers, selected);
                if !selected.existing_capacity
                    && matches!(
                        selected.provider,
                        ProviderId::Gcp | ProviderId::Aws | ProviderId::Azure
                    )
                {
                    if let (Some(remaining), Some(hourly)) = (
                        new_cloud_hourly_budget_usd.as_mut(),
                        selected.hourly_compute_usd,
                    ) {
                        *remaining = (*remaining - hourly).max(f64::default());
                    }
                    if let (Some(remaining), Some(cost)) = (
                        new_cloud_cost_budget_usd.as_mut(),
                        selected.expected_total_cost_usd,
                    ) {
                        *remaining = (*remaining - cost).max(f64::default());
                    }
                }
            }
            Ok(false) => {}
            Err(error) => {
                let _ =
                    super::storage::release_placement_lease(store, &job.job_id, &lease.token).await;
                return Err(error);
            }
        }
    }
    Ok(summary)
}

fn reserve_offer(offers: &mut [CapacityOffer], selected: &PlacementCandidate) {
    if let Some(offer) = offers.iter_mut().find(|offer| {
        offer.target_id == selected.target_id && offer.accelerator_type == selected.accelerator_type
    }) {
        offer.free_slots = offer.free_slots.saturating_sub(true as i64);
    }
}

fn payload_text<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| payload.get("diag")?.get(key)?.as_str())
}

async fn collect_offers(
    store: &JobStorage,
    cloud_providers: &[(String, Arc<dyn Provider>)],
    policy: &AutonomyPolicy,
) -> Result<(Vec<CapacityOffer>, BTreeMap<String, String>), StorageError> {
    let mut offers = Vec::new();
    let mut errors = BTreeMap::new();
    let consumers = capacity::read_consumer_capacity(store).await?;
    for (consumer_id, payload) in consumers {
        let Some(kind) = payload.get("kind").and_then(Value::as_str) else {
            continue;
        };
        let Some(provider) = crate::capabilities::provider(kind) else {
            continue;
        };
        if !policy.placement.allowed_providers.contains(&provider) {
            continue;
        }
        let free_vram = payload
            .get("free_vram_gb")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let slots = payload
            .get("free_slots")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if slots.is_empty() && free_vram > i64::default() {
            let accelerator = payload_text(&payload, "gpu_type").unwrap_or("");
            offers.push(CapacityOffer {
                target_id: consumer_id,
                provider,
                region: payload_text(&payload, "region")
                    .map(str::to_string)
                    .or_else(|| default_region(provider)),
                accelerator_type: accelerator.to_string(),
                machine_type: payload_text(&payload, "machine_type")
                    .map(str::to_string)
                    .or_else(|| {
                        sizing_for_accelerator(provider.as_str(), accelerator)
                            .map(|(_, machine)| machine)
                    })
                    .unwrap_or_default(),
                free_vram_gb: free_vram,
                free_slots: true as i64,
                existing: true,
            });
            continue;
        }
        for (accelerator, count) in slots {
            let count = count.as_i64().unwrap_or_default();
            if count <= i64::default() {
                continue;
            }
            offers.push(CapacityOffer {
                target_id: consumer_id.clone(),
                provider,
                region: payload_text(&payload, "region")
                    .map(str::to_string)
                    .or_else(|| default_region(provider)),
                accelerator_type: accelerator.clone(),
                machine_type: payload_text(&payload, "machine_type")
                    .map(str::to_string)
                    .or_else(|| {
                        sizing_for_accelerator(provider.as_str(), &accelerator)
                            .map(|(_, machine)| machine)
                    })
                    .unwrap_or_default(),
                free_vram_gb: free_vram,
                free_slots: count,
                existing: true,
            });
        }
    }
    for (name, provider) in cloud_providers {
        let Some(provider_id) = crate::capabilities::provider(name) else {
            continue;
        };
        if !policy.placement.allowed_providers.contains(&provider_id) {
            continue;
        }
        match crate::scheduler::quota::get_available_slots(store, provider.as_ref(), name).await {
            Ok(slots) => {
                for (accelerator, count) in slots {
                    if count <= i64::default() {
                        continue;
                    }
                    let Some((vram, machine)) = sizing_for_accelerator(name, &accelerator) else {
                        continue;
                    };
                    offers.push(CapacityOffer {
                        target_id: format!("{name}:new:{machine}"),
                        provider: provider_id,
                        region: default_region(provider_id),
                        accelerator_type: accelerator,
                        machine_type: machine,
                        free_vram_gb: vram,
                        free_slots: count,
                        existing: false,
                    });
                }
            }
            Err(error) => {
                errors.insert(name.clone(), error.to_string());
            }
        }
    }
    Ok((offers, errors))
}

fn candidates_for_job(
    job: &Job,
    offers: &[CapacityOffer],
    prices: &PriceBook,
    wall_times: &BTreeMap<(String, String), f64>,
    context: &CandidateContext<'_>,
) -> Vec<PlacementCandidate> {
    let policy = context.policy;
    offers
        .iter()
        .map(|offer| {
            let runtime = if job.runtime_seconds_estimate > f64::default() {
                job.runtime_seconds_estimate
            } else {
                crate::scheduler::cost::estimate_wall_time(
                    &job.command,
                    &offer.accelerator_type,
                    job.gpu_mem_gb,
                    wall_times,
                )
            };
            let preemptible = job.preemptible
                && policy.placement.allow_spot
                && (!policy.placement.require_checkpoint_for_spot
                    || job.max_preempts_before_ondemand > i64::default());
            let possible_regions = offer_regions(offer);
            let quote = possible_regions
                .iter()
                .filter_map(|region| {
                    prices.find_hourly(
                        offer.provider,
                        Some(region),
                        &offer.machine_type,
                        &offer.accelerator_type,
                        preemptible,
                    )
                })
                .max_by(|left, right| {
                    left.hourly_usd
                        .partial_cmp(&right.hourly_usd)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .or_else(|| {
                    prices.find_hourly(
                        offer.provider,
                        offer.region.as_deref(),
                        &offer.machine_type,
                        &offer.accelerator_type,
                        preemptible,
                    )
                });
            candidate(
                job,
                offer,
                runtime,
                preemptible,
                quote.as_ref(),
                &possible_regions,
                context,
            )
        })
        .collect()
}

fn candidate(
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
    if offer.free_slots <= i64::default() {
        rejected.push("no available slots".to_string());
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
            "provider fallback could leave required region {}",
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
        let fallback_region_forbidden = !offer.existing
            && possible_regions
                .iter()
                .any(|region| !policy.placement.allowed_regions.contains(region));
        if offered_region_forbidden || fallback_region_forbidden {
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
        available_slots: offer.free_slots,
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

async fn persist_predicted_savings(
    store: &JobStorage,
    decision_id: &str,
    job: &Job,
    selected: &PlacementCandidate,
    candidates: &[PlacementCandidate],
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    let Some(selected_cost) = selected.expected_total_cost_usd else {
        return Ok(());
    };
    let Some(baseline) = candidates
        .iter()
        .find(|candidate| candidate.eligible && candidate.expected_total_cost_usd.is_some())
    else {
        return Ok(());
    };
    let baseline_cost = baseline.expected_total_cost_usd.unwrap_or_default();
    if baseline.target_id == selected.target_id || baseline_cost <= selected_cost {
        return Ok(());
    }
    let record = SavingsRecord {
        schema_version: SCHEMA_VERSION,
        savings_id: uuid::Uuid::new_v4().to_string(),
        decision_id: decision_id.to_string(),
        resource_id: None,
        workload: Some(job.job_id.clone()),
        provider: selected.provider,
        measurement_started_at: now.to_rfc3339(),
        measurement_ended_at: None,
        baseline_cost_usd: baseline_cost,
        predicted_cost_usd: selected_cost,
        realized_cost_usd: None,
        predicted_savings_usd: baseline_cost - selected_cost,
        realized_savings_usd: None,
        confidence: TWO.recip(),
        source_invoice_period: None,
    };
    super::storage::write_savings(store, &record).await
}

async fn update_job_placement(
    store: &JobStorage,
    original: &Job,
    selected: &PlacementCandidate,
) -> Result<bool, StorageError> {
    let path = format!("queue/{}.json", original.job_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(false);
    };
    let mut current = Job::from_json(&versioned.content).map_err(|error| {
        StorageError::Other(format!("invalid queued job {}: {error}", original.job_id))
    })?;
    let assignment_matches = if selected.existing_capacity {
        current.assigned_to == selected.target_id
    } else {
        current.assigned_to.is_empty()
    };
    if current.provider == selected.provider.as_str()
        && current.pin_to_provider
        && assignment_matches
        && current.pinned_host.is_empty()
    {
        return Ok(false);
    }
    if !current.pinned_host.is_empty()
        && !selected
            .target_id
            .eq_ignore_ascii_case(&current.pinned_host)
    {
        return Ok(false);
    }
    current.provider = selected.provider.as_str().to_string();
    current.pin_to_provider = true;
    if selected.existing_capacity {
        current.assigned_to = selected.target_id.clone();
    } else {
        current.assigned_to.clear();
    }
    store
        .compare_and_swap_text(&path, &versioned.version, &current.to_json())
        .await?;
    store.refresh_job_metadata("queue", &current).await?;
    Ok(true)
}

async fn persist_unplaced_decision(
    store: &JobStorage,
    job: &Job,
    candidates: Vec<PlacementCandidate>,
    policy: &AutonomyPolicy,
    inventory_snapshot_id: &str,
) -> Result<(), StorageError> {
    let now = Utc::now();
    let expires = now + chrono::Duration::seconds(policy.limits.decision_ttl_seconds as i64);
    let decision = DecisionRecord {
        schema_version: SCHEMA_VERSION,
        decision_id: uuid::Uuid::new_v4().to_string(),
        kind: DecisionKind::Placement,
        subject_id: job.job_id.clone(),
        created_at: now.to_rfc3339(),
        expires_at: expires.to_rfc3339(),
        inventory_snapshot_id: inventory_snapshot_id.to_string(),
        policy_version: policy.policy_version.clone(),
        selected: None,
        candidates: candidates
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?,
        constraints: placement_constraints(job, policy),
        explanation: "No candidate satisfies every placement constraint".to_string(),
        lease_token: None,
        state: "blocked".to_string(),
    };
    super::storage::write_decision(store, &decision).await
}

fn placement_constraints(job: &Job, policy: &AutonomyPolicy) -> Vec<String> {
    let mut constraints = vec![
        format!("gpu_mem_gb >= {}", job.gpu_mem_gb),
        format!(
            "allowed_providers = {:?}",
            policy.placement.allowed_providers
        ),
    ];
    if !job.gpu_type.is_empty() {
        constraints.push(format!("gpu_type = {}", job.gpu_type));
    }
    if job.max_cost_per_hour_usd > f64::default() {
        constraints.push(format!("hourly_usd <= {:.6}", job.max_cost_per_hour_usd));
    }
    if job.pin_to_provider {
        constraints.push(format!("provider = {}", job.provider));
    }
    if !job.region.is_empty() {
        constraints.push(format!("region = {}", job.region));
    }
    constraints
}

fn explain_selection(selected: &PlacementCandidate, candidates: &[PlacementCandidate]) -> String {
    let rejected = candidates
        .iter()
        .filter(|candidate| !candidate.eligible)
        .count();
    format!(
        "Selected {} on {} at expected total ${:.6}; {} candidate(s) rejected by constraints",
        selected.target_id,
        selected.provider.as_str(),
        selected.expected_total_cost_usd.unwrap_or_default(),
        rejected
    )
}

fn job_order(left: &Job, right: &Job) -> std::cmp::Ordering {
    right.priority.cmp(&left.priority).then_with(|| {
        match (job_deadline(left), job_deadline(right)) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    })
}

fn job_deadline(job: &Job) -> Option<chrono::DateTime<Utc>> {
    job.deadline_at
        .as_deref()
        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .map(|deadline| deadline.with_timezone(&Utc))
}

fn candidate_order(left: &PlacementCandidate, right: &PlacementCandidate) -> std::cmp::Ordering {
    left.expected_total_cost_usd
        .unwrap_or(f64::INFINITY)
        .partial_cmp(&right.expected_total_cost_usd.unwrap_or(f64::INFINITY))
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            left.expected_finish_seconds
                .partial_cmp(&right.expected_finish_seconds)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| left.target_id.cmp(&right.target_id))
}

fn observed_startup_seconds(
    feedback: &[super::storage::PlacementFeedback],
    target: &str,
) -> Option<f64> {
    median(
        feedback
            .iter()
            .filter(|entry| entry.target_id == target)
            .filter_map(|entry| entry.startup_seconds)
            .collect(),
    )
}

fn machine_capacity(provider: ProviderId, machine_type: &str) -> Option<(i64, i64)> {
    if provider != ProviderId::Gcp {
        return None;
    }
    let cpu = machine_type.rsplit('-').next()?.parse::<i64>().ok()?;
    let two = (u16::BITS / u8::BITS) as i64;
    let memory_per_cpu = if machine_type.contains("highmem") {
        u8::BITS as i64
    } else if machine_type.contains("highcpu") {
        true as i64
    } else if machine_type.contains("standard") {
        two * two
    } else {
        return None;
    };
    Some((cpu, cpu * memory_per_cpu))
}

fn observed_failure_probability(
    feedback: &[super::storage::PlacementFeedback],
    target: &str,
) -> Option<f64> {
    let samples: Vec<_> = feedback
        .iter()
        .filter(|entry| entry.target_id == target)
        .collect();
    if samples.is_empty() {
        return None;
    }
    let failed = samples.iter().filter(|entry| !entry.succeeded).count();
    Some(failed as f64 / samples.len() as f64)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    Some(values[values.len() / (u16::BITS / u8::BITS) as usize])
}

fn sizing_for_accelerator(provider: &str, accelerator: &str) -> Option<(i64, String)> {
    crate::catalog::GPU_SIZING
        .get(provider)?
        .iter()
        .find(|(_, (_, candidate))| *candidate == accelerator)
        .map(|(vram, (machine, _))| (*vram, (*machine).to_string()))
}

fn default_region(provider: ProviderId) -> Option<String> {
    match provider {
        ProviderId::Gcp => crate::config::zone_rotation()
            .first()
            .and_then(|zone| zone.rsplit_once('-').map(|(region, _)| region.to_string())),
        ProviderId::Azure => crate::config::azure_locations().first().cloned(),
        ProviderId::Aws => Some(crate::config::aws_region().to_string()),
        _ => None,
    }
}

fn offer_regions(offer: &CapacityOffer) -> BTreeSet<String> {
    let mut regions = if offer.existing {
        offer.region.iter().cloned().collect()
    } else {
        match offer.provider {
            ProviderId::Gcp => {
                let zones = crate::config::machine_type_zones()
                    .get(&offer.machine_type)
                    .map(Vec::as_slice)
                    .unwrap_or_else(|| crate::config::zone_rotation());
                zones
                    .iter()
                    .filter_map(|zone| zone.rsplit_once('-').map(|(region, _)| region.to_string()))
                    .collect()
            }
            ProviderId::Azure => crate::config::azure_locations().iter().cloned().collect(),
            ProviderId::Aws => BTreeSet::from([crate::config::aws_region().to_string()]),
            _ => BTreeSet::new(),
        }
    };
    if regions.is_empty() {
        regions.extend(offer.region.iter().cloned());
    }
    regions
}

fn crosses_provider_boundary(job: &Job, provider: ProviderId) -> bool {
    let uris = [&job.startup_script_uri, &job.output_uri];
    uris.iter().any(|uri| {
        (!uri.is_empty())
            && ((uri.starts_with("gs://") && provider != ProviderId::Gcp)
                || (uri.starts_with("s3://") && provider != ProviderId::Aws)
                || (uri.starts_with("https://")
                    && uri.contains("blob.core.windows.net")
                    && provider != ProviderId::Azure))
    })
}

