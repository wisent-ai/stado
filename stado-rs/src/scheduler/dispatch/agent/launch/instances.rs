//! The per-bucket create_instance loop: quota and time budgets, one render
//! per bucket, and stockout-aware tier escalation.

use std::collections::BTreeMap;
use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::catalog::GPU_SIZING;
use crate::config;
use crate::providers::{Provider, ProviderError};
use crate::queue::JobStorage;
use crate::scheduler::scheduler::{log, SchedulerError};
use crate::sizing::Sizing;

use super::super::startup::render_agent_startup_script;
use super::buckets::bucket_jobs;
use super::inputs::AgentDispatchInputs;

/// [`dispatch_agent_vms`] with the startup-script template and deployment
/// settings injected so tests remain independent of ambient configuration.
///
/// [`dispatch_agent_vms`]: super::dispatch_agent_vms
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_agent_vms_with_template(
    inputs: AgentDispatchInputs<'_>,
    template: &str,
    store: &JobStorage,
    sizing: &Sizing,
    provider: &dyn Provider,
    provider_name: &str,
    secrets: &BTreeMap<String, String>,
    deployment: &BTreeMap<String, String>,
    now_utc: DateTime<Utc>,
) -> Result<i64, SchedulerError> {
    let AgentDispatchInputs {
        queued,
        yield_targets,
        available,
        accel_dispatched,
        per_accel_share,
        per_tick_cap,
        scheduled_so_far,
    } = inputs;
    let buckets = bucket_jobs(
        &queued,
        &yield_targets,
        provider_name,
        sizing,
        store,
        now_utc,
    )
    .await?;

    let protected_agent_grant = if matches!(
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::Execution, provider_name,)
            .map(|variant| variant.adapter),
        Some(crate::capabilities::RuntimeAdapter::Execution(
            crate::capabilities::ExecutionAdapter::Azure
        ))
    ) {
        Some(
            secrets
                .get(crate::coordinator::AZURE_AGENT_PROTECTED_GRANT)
                .filter(|grant| !grant.is_empty())
                .map(String::as_str)
                .ok_or_else(|| {
                    ProviderError::Value(
                        "Azure agent dispatch requires a dedicated grant for protected-settings delivery"
                            .to_string(),
                    )
                })?,
        )
    } else {
        None
    };
    let tick_tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut created: i64 = 0;
    let mut scheduled = scheduled_so_far;
    // Time budget: each create_instance can spend ~10s/zone × 7+ zones
    // for first-encounter stockouts, plus a full retry on the larger tier
    // in the escalation branch. With n_to_dispatch=2-3 per bucket, the
    // autoscaler can easily eat 300+ seconds — confirmed live 03:39Z
    // 2026-05-15 tick 504'd at 540s. Bail out after 120s in the
    // dispatcher and let the next tick try again (caches will be warm).
    const DISPATCH_BUDGET_S: u64 = 120;
    let start = Instant::now();
    'buckets: for ((accel, mt), jobs) in &buckets {
        if scheduled >= per_tick_cap {
            break;
        }
        if start.elapsed().as_secs() > DISPATCH_BUDGET_S {
            log(&format!(
                "dispatch budget exhausted after {scheduled} scheduled; deferring remaining buckets to next tick"
            ));
            break;
        }
        let quota_left = available.get(accel).copied().unwrap_or(0);
        if quota_left <= 0 {
            log(&format!(
                "Skip bucket accel={accel} machine={mt}: 0 quota slots"
            ));
            continue;
        }
        let share_left = per_accel_share - accel_dispatched.get(accel).copied().unwrap_or(0);
        if share_left <= 0 {
            continue;
        }
        let n_to_dispatch = (jobs.len() as i64)
            .min(quota_left)
            .min(share_left)
            .min(per_tick_cap - scheduled);
        let biggest = jobs
            .iter()
            .max_by_key(|j| j.gpu_mem_gb)
            .expect("bucket is non-empty");
        // No-preemptible policy: per user instruction (2026-05-06), this
        // codebase is NOT to dispatch Spot/preemptible VMs even when the
        // job's `preemptible` field is True. Repeated Spot reclaims of
        // A100-80 capacity in us-central1 caused 8 cloud-agent VMs to be
        // deleted under instance_termination_action=DELETE in a single
        // 3-second window (22:21:10-13Z), forcing requeues that burned
        // restart-budget on misclassified jobs (since fixed in 0.4.55,
        // but the underlying preemption noise persists). Override the
        // job-level flag and force every dispatch to STANDARD.
        let preemptible_for_call = false;
        // Render once per bucket: the script depends only on the template,
        // the bucket's accel and the substitution maps, never on the
        // instance index. A template whose placeholders cannot all be
        // filled stops dispatch here — before any create_instance — rather
        // than booting VMs that `set -u` kills on their first export.
        let script = match render_agent_startup_script(
            provider_name,
            template,
            accel,
            secrets,
            deployment,
        ) {
            Ok(script) => script,
            Err(exc) => {
                log(&format!(
                    "REFUSING to dispatch agent VMs for accel={accel} machine={mt}: {exc}. \
                     No instance was created; fix the coordinator env/config and the next \
                     tick retries."
                ));
                break 'buckets;
            }
        };
        if protected_agent_grant.is_some_and(|grant| script.contains(grant)) {
            return Err(ProviderError::Value(
                "refusing Azure dispatch because the protected agent grant reached customData"
                    .to_string(),
            )
            .into());
        }
        for i in 0..n_to_dispatch {
            if start.elapsed().as_secs() > DISPATCH_BUDGET_S {
                log(&format!(
                    "dispatch budget exhausted mid-bucket {accel}; deferring"
                ));
                break 'buckets;
            }
            let instance_name = format!(
                "{}-agent-{}-{tick_tag}-{i}",
                config::INSTANCE_PREFIX,
                accel.rsplit('-').next().unwrap_or(accel)
            );
            let mut effective_accel = accel.clone();
            let mut ref_opt = provider
                .create_agent_instance(
                    &instance_name,
                    mt,
                    accel,
                    biggest.boot_disk_gb,
                    &biggest.image,
                    &biggest.image_project,
                    &script,
                    preemptible_for_call,
                    protected_agent_grant,
                )
                .await?;
            if ref_opt.is_none() {
                log(&format!(
                    "Agent VM create failed accel={accel} machine={mt}"
                ));
                // Stockout-aware escalation: when create_instance returns
                // None (zone STOCKOUTs across all configured zones for
                // this accel), try the next-larger tier from GPU_SIZING.
                // The job is larger than needed but routes around the
                // capacity shortage. The same VM tier returns on next
                // tick if the operator hasn't manually re-routed.
                let pmem = biggest.gpu_mem_gb;
                let mut escalated = false;
                if let Some(sizing_map) = GPU_SIZING.get(provider_name) {
                    for (next_mem, (next_mt, next_accel)) in sizing_map.range(pmem + 1..) {
                        if next_accel == &accel.as_str() && next_mt == &mt.as_str() {
                            continue;
                        }
                        if available.get(*next_accel).copied().unwrap_or(0) <= 0 {
                            continue;
                        }
                        log(&format!(
                            "escalating {accel}/{mt} -> {next_accel}/{next_mt} \
                             (stockout on {accel}, next tier mem={next_mem})"
                        ));
                        ref_opt = provider
                            .create_agent_instance(
                                &instance_name,
                                next_mt,
                                next_accel,
                                biggest.boot_disk_gb,
                                &biggest.image,
                                &biggest.image_project,
                                &script,
                                preemptible_for_call,
                                protected_agent_grant,
                            )
                            .await?;
                        if ref_opt.is_some() {
                            effective_accel = next_accel.to_string();
                            escalated = true;
                            break;
                        }
                    }
                }
                if !escalated {
                    continue;
                }
            }
            let instance_ref = ref_opt.expect("escalated or initial create returned a ref");
            *available.entry(effective_accel.clone()).or_insert(0) -= 1;
            *accel_dispatched.entry(effective_accel.clone()).or_insert(0) += 1;
            scheduled += 1;
            created += 1;
            log(&format!(
                "Dispatched agent VM {instance_ref} accel={effective_accel} machine={mt} \
                 preemptible={preemptible_for_call}"
            ));
            if scheduled >= per_tick_cap {
                break;
            }
        }
    }
    Ok(created)
}
