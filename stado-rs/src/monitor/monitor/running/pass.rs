//! The per-running-job pass: finalize the two terminal statuses, then decide
//! liveness for everything else — boot grace first, then the local@ agent
//! branches, then the cloud instance's own existence and lifecycle state.

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::Value;

use crate::config;
use crate::models::{isoformat_utc, job_state};
use crate::monitor::alerts::send_alert;
use crate::monitor::heartbeat_guard as hg;
use crate::providers::Provider;
use crate::queue::capacity::read_consumer_capacity;
use crate::queue::JobStorage;

use super::super::lifecycle::{requeue, requeue_dead_local_host_orphan, requeue_preempted};
use super::super::{elapsed_seconds, log, MonitorError};
use super::vm_delete::safe_delete_vm_by_hostname;

/// Check all running jobs. Handle completion, failure, preemption, stale.
pub async fn check_running_jobs(
    store: &JobStorage,
    provider: &dyn Provider,
) -> Result<(), MonitorError> {
    let running = store.list_jobs("running", 0).await?;
    log(&format!("Checking {} running jobs", running.len()));

    // Lazily-built per-call caches (Python: _live_consumers_cache /
    // _running_vm_names_cache, built at most once per invocation).
    let mut live_consumers_cache: Option<BTreeMap<String, Value>> = None;
    let mut running_vm_names_cache: Option<BTreeMap<String, String>> = None;

    for mut job in running {
        let job_id = job.job_id.clone();
        let Some(instance_ref) = job.instance_ref.clone().filter(|r| !r.is_empty()) else {
            requeue(store, &mut job, "no instance ref").await?;
            continue;
        };

        let status = store.read_status(&job_id).await?;

        if status.as_deref() == Some("COMPLETED") {
            job.state = job_state::COMPLETED.to_string();
            job.completed_at = Some(isoformat_utc(Utc::now()));
            provider.delete_instance(&instance_ref).await?;
            store.move_job(&job, "running", "completed").await?;
            store.cleanup_status(&job_id).await?;
            log(&format!("{job_id}: COMPLETED"));
        } else if status.as_deref() == Some("FAILED") {
            job.state = job_state::FAILED.to_string();
            job.failed_at = Some(isoformat_utc(Utc::now()));
            provider.delete_instance(&instance_ref).await?;
            store.move_job(&job, "running", "failed").await?;
            store.cleanup_status(&job_id).await?;
            let msg = format!(
                "Job {job_id} FAILED: {}",
                job.command.chars().take(100).collect::<String>()
            );
            // Best-effort: alert failures never block the monitor tick.
            send_alert(config::alerts_topic(), &msg, "").await;
            log(&format!("{job_id}: FAILED"));
        } else {
            // Boot grace: a freshly (re)dispatched job has not yet
            // written its first heartbeat (agent claim -> apt/clone/pip/
            // multi-GB ckpt-pull preamble before slots.py Popen +
            // _write_heartbeat) while the previous run's heartbeat blob
            // is already aged, so the orphan / VM-gone staleness guards
            // below false-positive and requeue a healthy starting job
            // (synchronized 3ef705b2+724084db requeues 16:18/20:00/
            // 21:33/21:57 on RUNNING 0.4.228 VMs). Skip requeue logic
            // until the job has had BOOT_GRACE_SECONDS to heartbeat.
            if let Some(sa) = job.started_at.as_deref().filter(|s| !s.is_empty()) {
                // Python: except (ValueError, TypeError) -> pass (parse
                // errors reach the guards below).
                if let Some(started) = hg::parse_iso_lenient(sa) {
                    if elapsed_seconds(Utc::now(), started) < 1800.0 {
                        continue;
                    }
                }
            }
            if let Some(hostname) = instance_ref.strip_prefix("local@") {
                if live_consumers_cache.is_none() {
                    live_consumers_cache = Some(read_consumer_capacity(store).await?);
                }
                let live = live_consumers_cache.as_ref().expect("just built");
                let agent_live = crate::capabilities::get("execution")
                    .into_iter()
                    .flat_map(|capability| capability.variants)
                    .any(|variant| live.contains_key(&format!("{}-{hostname}", variant.id)));
                if agent_live {
                    // Agent up != this old job progresses (restarts
                    // orphan it). Heartbeat is proof; self-terminating
                    // cmds (pkill wc agent) -> kill IS success.
                    if hg::any_job_heartbeat_fresh(store, std::slice::from_ref(&job_id), 1800.0)
                        .await
                    {
                        continue;
                    }
                    if hg::finalize_if_self_terminating(store, &mut job, &log).await? {
                        continue;
                    }
                    if !hg::any_job_checkpoint_fresh(store, &job, 5400.0).await
                        && requeue(
                            store,
                            &mut job,
                            "local agent live but job heartbeat stale (orphan)",
                        )
                        .await?
                    {
                        let cache = running_vm_names_cache.clone().unwrap_or_default();
                        safe_delete_vm_by_hostname(provider, hostname, &cache).await;
                    }
                    continue;
                }
                if hostname.starts_with("wisent-agent-") {
                    if running_vm_names_cache.is_none() {
                        running_vm_names_cache = Some(
                            provider
                                .list_running_instance_refs_with_age()
                                .await?
                                .into_iter()
                                .map(|(r, _age)| (r.split('@').next().unwrap_or("").to_string(), r))
                                .collect(),
                        );
                    }
                    let cache = running_vm_names_cache.as_ref().expect("just built");
                    if !cache.contains_key(hostname) {
                        // fresh job heartbeat = VM+agent+training alive;
                        // aggregated_list missed a transient non-RUNNING
                        // (STAGING/REPAIRING/live-migration) snapshot
                        if hg::any_job_heartbeat_fresh(store, std::slice::from_ref(&job_id), 1800.0)
                            .await
                        {
                            continue;
                        }
                        let moved = if job.preemptible {
                            requeue_preempted(
                                store,
                                &mut job,
                                "Spot preempted (cloud agent gone)",
                                true,
                            )
                            .await?
                        } else {
                            requeue(store, &mut job, "VM gone (cloud agent missing from fleet)")
                                .await?
                        };
                        if moved {
                            safe_delete_vm_by_hostname(provider, hostname, cache).await;
                        }
                        continue;
                    }
                }
                requeue_dead_local_host_orphan(store, &mut job, &job_id).await?;
                continue;
            }

            let alive = provider.instance_exists(&instance_ref).await?;
            let lifecycle = provider.instance_lifecycle_state(&instance_ref).await?;

            if !alive && lifecycle.as_deref() == Some("TERMINATED") && job.preemptible {
                if requeue_preempted(store, &mut job, "Spot preempted", false).await? {
                    provider.delete_instance(&instance_ref).await?;
                }
            } else if !alive {
                // Python f-string renders a None lifecycle as "None".
                let lifecycle_str = lifecycle.as_deref().unwrap_or("None");
                if requeue(
                    store,
                    &mut job,
                    &format!("instance gone (lifecycle={lifecycle_str})"),
                )
                .await?
                {
                    provider.delete_instance(&instance_ref).await?;
                }
            }
        }
    }
    Ok(())
}
