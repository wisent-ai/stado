//! The per-bucket create_instance loop: quota and time budgets, one render
//! per bucket, and stockout-aware tier escalation.

use std::collections::BTreeMap;
use std::ops::Bound;

use crate::catalog::GPU_SIZING;
use crate::config;
use crate::primitives::tally::Tally;
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
    let buckets = bucket_jobs(&queued, &yield_targets, provider_name, sizing, store).await?;

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
    // A clock before the epoch names no tick: instances tagged with it would
    // collide with every other tick's names, so the tick is refused instead.
    let tick_tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            ProviderError::Value(format!(
                "refusing to name agent instances: the coordinator's clock reads before the Unix epoch ({error})"
            ))
        })?
        .as_secs();
    let mut scheduled = scheduled_so_far;
    'buckets: for ((accel, mt), jobs) in &buckets {
        if scheduled >= per_tick_cap {
            break;
        }
        // A quota nobody read is not an empty one: the bucket waits for the
        // next read, and the log says which it was.
        let Some(&quota_left) = available.get(accel) else {
            log(&format!(
                "Skip bucket accel={accel} machine={mt}: its quota was not read this tick"
            ));
            continue;
        };
        if !quota_left.is_positive() {
            log(&format!(
                "Skip bucket accel={accel} machine={mt}: {quota_left} quota slots left"
            ));
            continue;
        }
        let share_left = match accel_dispatched.get(accel) {
            Some(dispatched) => per_accel_share - dispatched,
            // Nothing was dispatched on this accelerator yet this tick.
            None => per_accel_share,
        };
        if !share_left.is_positive() {
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
        // The machine's disk is the largest one a job in the bucket stated at
        // submit (`--boot-disk-gb`). Stado assumes no size: a bucket where no
        // job stated one rents nothing, and its jobs wait for a registered
        // host or a resubmission that states the disk.
        let Some(boot_disk_gb) = jobs
            .iter()
            .map(|job| job.boot_disk_gb)
            .filter(|disk| disk.is_positive())
            .max()
        else {
            log(&format!(
                "REFUSING to rent agent machines for accel={accel} machine={mt}: none of its \
                 {} queued job(s) states a boot disk (stado submit --boot-disk-gb), and Stado \
                 assumes no size",
                jobs.len()
            ));
            continue;
        };
        // Fleet policy dispatches standard instances even when a job requests
        // preemption, avoiding provider reclamation of an active agent.
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
        let count = usize::try_from(n_to_dispatch).map_err(|error| {
            ProviderError::Value(format!(
                "the {n_to_dispatch} agent instances bucket accel={accel} machine={mt} may create is not a count: {error}"
            ))
        })?;
        let tier = match accel.rsplit_once('-') {
            Some((_, tier)) => tier,
            None => accel.as_str(),
        };
        for (i, _) in jobs.iter().take(count).enumerate() {
            let instance_name = format!("{}-agent-{tier}-{tick_tag}-{i}", config::INSTANCE_PREFIX);
            let mut effective_accel = accel.clone();
            let mut ref_opt = provider
                .create_agent_instance(
                    &instance_name,
                    mt,
                    accel,
                    boot_disk_gb,
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
                    for (next_mem, (next_mt, next_accel)) in
                        sizing_map.range((Bound::Excluded(pmem), Bound::Unbounded))
                    {
                        if next_accel == &accel.as_str() && next_mt == &mt.as_str() {
                            continue;
                        }
                        // A tier whose quota was not read is not escalated to.
                        if !available
                            .get(*next_accel)
                            .is_some_and(|left| left.is_positive())
                        {
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
                                boot_disk_gb,
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
            available
                .get_mut(&effective_accel)
                .expect("a tier is dispatched on only after its quota was read")
                .retract();
            accel_dispatched
                .entry(effective_accel.clone())
                .or_default()
                .tally();
            scheduled.tally();
            log(&format!(
                "Dispatched agent VM {instance_ref} accel={effective_accel} machine={mt} \
                 preemptible={preemptible_for_call}"
            ));
            if scheduled >= per_tick_cap {
                break;
            }
        }
    }
    Ok(scheduled - scheduled_so_far)
}
