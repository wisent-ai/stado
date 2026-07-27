//! Coordinator daemon — Rust port of `stado/coordinator.py`.
//!
//! The same scheduling tick the GCP Cloud Function runs, as a long-lived
//! local process so the system can run without GCP CF / Cloud Scheduler.
//!
//! Reads the named coordinator entry from the registry (default: the one
//! whose active=true), constructs the same JobStorage + provider +
//! scheduler call chain the CF's `monitor_jobs` uses, and loops on the
//! configured interval_seconds. `--once` runs a single tick and exits
//! (cron-driven runtimes).
//!
//! State stays in the registry-declared state_uri (currently always GCS),
//! so swapping coordinator from GCF to a daemon on the Mac doesn't change
//! which queue the agents see.
//!
//! Cloud Function parity: `stado/cloud_function/main.py::monitor_jobs`
//! composes the SAME tick (fire due schedules -> normalize sizing ->
//! makespan assign -> per provider check/reap/schedule -> run reaper ->
//! billing collect) and needs no separate port — [`run_tick`] is the single
//! implementation. Confirmed against main.py: the only differences are the
//! secrets source (CF: Secret Manager; daemon: process env, as in
//! coordinator.py) and the box owner default (CF: "gcp-cloud-function";
//! daemon: hostname).
//!
//! Registry re-resolution is at full Python parity: every tick re-reads the
//! canonical registry (source="gcs") via
//! [`crate::targets::fetch_registry_remote`] so an operator can kill a rogue
//! daemon by removing its entry. The remote registry is the ONLY authority
//! for the self-survival check — there is no local escape hatch.
//!
//! DEVIATION from Python: the check exits ONLY on a registry that was
//! successfully READ and does not list the coordinator. Python treats an
//! unreachable store as an empty registry, so a storage outage terminates
//! every coordinator in the fleet at once — exactly what happened when the
//! GCP billing account was closed and every GCS call began answering
//! `accountDisabled`. A fetch failure now logs loudly and keeps ticking.
//!
//! Deviations from coordinator.py (all deliberate):
//! 1. Self-update: Python does PyPI drift-detect + `pip install --upgrade`
//!    + `os.execv` each tick. The Rust binary cannot pip-upgrade itself, so
//!      drift only logs a warning (same stance as
//!      providers/local/version_check.rs). TODO(phase-4): binary self-update.
//! 2. Billing: coordinator.py's daemon tick never collected billing (only
//!    the CF did). Per the port spec the tick includes the billing
//!    collector (fault-isolated, matching the CF), behind a flag so tests
//!    stay hermetic.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::config;
use crate::monitor::billing::collect_billing;
use crate::monitor::monitor::{check_running_jobs, reap_dead_agents, MonitorError};
use crate::monitor::reap::reap_terminal_runs;
use crate::providers::{get_provider, BoxProvider, Provider};
use crate::queue::{JobStorage, StorageError};
use crate::scheduler::dispatch::r#box::run_box_tick;
use crate::scheduler::makespan::assign_jobs;
use crate::scheduler::scheduler::{schedule_queued_jobs, SchedulerError};
use crate::schedules::fire_due_schedules;
use crate::targets::{fetch_registry_remote, load_registry_auto, Coordinator};

/// `[tick] ...` — the coordinator's log prefix (Python `_log`).
fn log(msg: &str) {
    eprintln!("[tick] {msg}");
}

/// Tick failure.
#[derive(Debug, thiserror::Error)]
pub enum CoordinatorError {
    /// Storage failures from any tick phase.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Scheduler failures from `schedule_queued_jobs`.
    #[error(transparent)]
    Scheduler(#[from] SchedulerError),
    /// Monitor failures from `check_running_jobs` / `reap_dead_agents`.
    #[error(transparent)]
    Monitor(#[from] MonitorError),
}

/// One resolved provider arm of the tick. Box providers need their
/// concrete type for [`run_box_tick`], so they cannot hide behind
/// `Arc<dyn Provider>` here.
pub enum ResolvedProvider {
    /// A cloud VM provider (gcp/aws/azure): check + reap + schedule.
    Cloud {
        /// Provider name from `WC_PROVIDERS` (also the reaper `kind`).
        name: String,
        /// The provider client.
        provider: Arc<dyn Provider>,
    },
    /// A Box provider (box/box-ascii): the lease state machine tick.
    Box {
        /// Provider name from `WC_PROVIDERS`.
        name: String,
        /// The concrete box provider.
        provider: Arc<BoxProvider>,
    },
}

/// Pick the coordinator entry: explicit --target, or the active one
/// (Python `_resolve_coordinator`; source="auto" — GCS first, bundled
/// fallback).
async fn resolve_coordinator(target: Option<&str>) -> Result<Coordinator, String> {
    let registry = load_registry_auto().await.map_err(|exc| exc.to_string())?;
    if let Some(target) = target {
        return registry
            .lookup_coordinator(target)
            .cloned()
            .ok_or_else(|| format!("coordinator '{target}' not found in registry"));
    }
    let active: Vec<&Coordinator> = registry.coordinators.iter().filter(|c| c.active).collect();
    if active.is_empty() {
        return Err(
            "no active coordinator in registry. Set active=true on one entry \
             or pass --target NAME explicitly."
                .into(),
        );
    }
    if active.len() > 1 {
        let names = active
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "multiple active coordinators ({names}); set active=true on exactly one"
        ));
    }
    Ok(active[0].clone())
}

/// Strip 'gs://' prefix to get the bucket name JobStorage expects
/// (Python `_bucket_from_state_uri`).
fn bucket_from_state_uri(state_uri: &str) -> String {
    state_uri
        .strip_prefix("gs://")
        .unwrap_or(state_uri)
        .split('/')
        .next()
        .unwrap_or("")
        .to_string()
}

/// `platform.node()` — the daemon-side default box-tick owner (Python
/// `os.uname().nodename`). Same approach as queue/submit.rs.
fn nodename() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return name;
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_default()
}

/// Populate the secrets map that dispatch_agent_vms uses to fill
/// ${KEY} placeholders in startup_gpu_agent.sh. Without this, the
/// rendered script keeps a literal ${HF_TOKEN}; with `set -u` at the
/// top of the template, line 50 (`export HF_TOKEN="${HF_TOKEN}"`)
/// crashes on unbound variable, the agent never starts, and the VM
/// sits idle until manually deleted. We saw 37 such orphan VMs
/// accumulate over ~24h on 2026-05-09 -> 2026-05-10 because secrets
/// had been an empty dict here forever.
///
/// Credentials only. The non-secret `${KEY}` substitutions the templates
/// also need — storage backend, Azure account/container, release base URL,
/// AWS bucket and region — are produced by
/// [`crate::scheduler::dispatch::agent::deployment_substitutions`] from
/// config, so a deployment setting never has to be smuggled through a
/// secrets bag, and a missing one now aborts the bucket instead of booting
/// a VM that dies on `set -u`.
///
/// Crate-visible so `crate::doctor` renders its template preflight with the
/// exact secrets bag a real tick would supply; a preflight fed a different
/// map could pass while dispatch still shipped an unsubstituted `${KEY}`.
pub(crate) fn secrets_from_env() -> BTreeMap<String, String> {
    let mut secrets = BTreeMap::new();
    for key in ["HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"] {
        if let Ok(val) = std::env::var(key) {
            let val = val.trim();
            if !val.is_empty() {
                secrets.insert(key.to_string(), val.to_string());
            }
        }
    }
    if let Some(hf) = secrets.get("HF_TOKEN").cloned() {
        secrets
            .entry("HUGGING_FACE_HUB_TOKEN".to_string())
            .or_insert(hf);
    }
    // Supabase Management API token goes to a distinct placeholder
    // (WC_SUPABASE_TOKEN) so the startup template can use bash empty-
    // default expansion without conflicting with python's literal-string
    // substitution. dispatched VMs export SUPABASE_ACCESS_TOKEN in their
    // env when this is populated.
    if let Ok(supa) = std::env::var("SUPABASE_ACCESS_TOKEN") {
        let supa = supa.trim();
        if !supa.is_empty() {
            secrets.insert("WC_SUPABASE_TOKEN".to_string(), supa.to_string());
        }
    }
    if !secrets.contains_key("HF_TOKEN") {
        log(
            "WARN: HF_TOKEN not in coordinator env; dispatched VMs will \
             fail their startup script on `set -u` line 50. Set HF_TOKEN \
             in the LaunchAgent's EnvironmentVariables and reload.",
        );
    }
    secrets
}

/// Resolve `WC_PROVIDERS` into tick arms. "local" is skipped (device-local
/// agents claim assigned jobs directly; there is no cloud VM lifecycle to
/// schedule or reap for that provider). A constructor failure is logged
/// and skipped so a misconfigured provider never blocks the primary one
/// (Python wraps the box arm in try/except; the cloud arms construct
/// lazily and cannot fail here).
pub fn resolve_providers() -> Vec<ResolvedProvider> {
    let mut out = Vec::new();
    for name in config::wc_providers() {
        match name.as_str() {
            "local" => continue,
            "box" | "box-ascii" => match BoxProvider::from_env() {
                Ok(provider) => out.push(ResolvedProvider::Box {
                    name: name.clone(),
                    provider: Arc::new(provider),
                }),
                Err(exc) => log(&format!("provider {name} tick failed: {exc}")),
            },
            _ => match get_provider(name) {
                Ok(provider) => out.push(ResolvedProvider::Cloud {
                    name: name.clone(),
                    provider,
                }),
                Err(exc) => log(&format!("provider {name} tick failed: {exc}")),
            },
        }
    }
    out
}

/// One scheduling cycle across every provider (Python
/// `coordinator._run_tick` + the CF's billing tail).
///
/// Each provider gets its own check_running_jobs + schedule_queued_jobs
/// pass; the queue is shared (state lives in JobStorage), so a
/// pin_to_provider job lands wherever its provider field points and an
/// unpinned job is offered to whichever provider claims first.
///
/// `with_billing` runs the billing-credits collector at the end (the CF
/// behavior; coordinator.py's daemon never billed). Tests pass `false`
/// to stay hermetic — the collector talks to BigQuery/ARM.
pub async fn run_tick(
    store: &JobStorage,
    secrets: &BTreeMap<String, String>,
    providers: &[ResolvedProvider],
    with_billing: bool,
    log: &dyn Fn(&str),
) -> Result<i64, CoordinatorError> {
    if let Err(exc) = config::refresh_model_policy(store).await {
        log(&format!(
            "model policy refresh failed; retaining last good policy: {exc}"
        ));
    }
    // Fire recurring (cron) schedules FIRST so any job submitted this tick
    // is visible to the assignment + dispatch passes below, instead of
    // waiting a full interval_seconds to be picked up.
    let n_fired = fire_due_schedules(store, log, Utc::now()).await?;
    if n_fired > 0 {
        log(&format!("schedules: fired {n_fired} due schedule(s)"));
    }
    // Coordinator-authoritative sizing: re-zero any queued job whose model
    // has no measured peak (stamp the measured peak if one exists) BEFORE
    // assignment. A pre-0.4.237 agent that requeues a job writes the old
    // hardcoded estimate_gpu_memory value back; makespan's assigned_to-only
    // write then preserves it. Correcting it here each tick makes the
    // coordinator the single sizing authority instead of waiting for
    // fleet-wide drift.
    let n_sized = crate::sizing::global()
        .normalize_queue_sizing(store, log)
        .await?;
    if n_sized > 0 {
        log(&format!(
            "sizing: corrected {n_sized} stale queue gpu_mem_gb values"
        ));
    }
    // Centralized makespan-minimizing matcher. Writes assigned_to back to
    // the queue blob; the agent side refuses jobs pinned to a different
    // agent. Priority stays user-controlled — it's a queue-order knob,
    // not a fleet-routing one.
    let n_assigned = assign_jobs(store, log).await?;
    if n_assigned > 0 {
        log(&format!(
            "assignment: matched {n_assigned} queued jobs to agents"
        ));
    }
    let mut total: i64 = 0;
    for arm in providers {
        match arm {
            ResolvedProvider::Box { name, provider } => {
                let owner = std::env::var("WC_COORDINATOR_ID").unwrap_or_else(|_| nodename());
                match run_box_tick(store, provider, &owner).await {
                    Ok(n) => total += n,
                    Err(exc) => log(&format!("provider {name} tick failed: {exc}")),
                }
            }
            ResolvedProvider::Cloud { name, provider } => {
                check_running_jobs(store, provider.as_ref()).await?;
                let reaped = reap_dead_agents(store, provider.as_ref(), name).await?;
                if reaped > 0 {
                    log(&format!("{name}: reaped {reaped} dead-agent VM(s)"));
                }
                total += schedule_queued_jobs(store, provider.as_ref(), name, secrets).await?;
            }
        }
    }
    // By-run reaper: drop per-job blobs once a run is fully terminal so
    // completed/+failed/ stop accumulating thousands of orphaned records.
    // Capped per tick to bound work on a large backlog.
    let summary = reap_terminal_runs(store, config::RUN_REAP_PER_TICK).await?;
    if summary.reaped_runs > 0 {
        log(&format!(
            "run-reaper: reaped {} run(s), deleted {} job blob(s)",
            summary.reaped_runs, summary.deleted_jobs
        ));
    }
    if with_billing {
        // Billing-credits collector. Global (not per-provider), runs last
        // and is fully fault-isolated internally: each source's exact error
        // is captured into the JSON blob (and the upload itself only logs),
        // so a broken collector never aborts the dispatch tick that the
        // drain depends on (CF behavior; Python coordinator.py's daemon
        // never billed).
        collect_billing(store).await;
    }
    Ok(total)
}

/// Coordinator daemon entry point (Python `coordinator.run`). Returns the
/// process exit code; `Err` is a SystemExit-style fatal message.
pub async fn run(target: Option<&str>, once: bool) -> Result<i32, String> {
    let coord = resolve_coordinator(target).await?;
    if coord.runtime == "gcp_cloud_function" {
        log(&format!(
            "coordinator '{}' runtime=gcp_cloud_function: tick is driven by \
             Cloud Scheduler, this daemon is a no-op. Use --target to point \
             at a runtime=daemon entry instead.",
            coord.name
        ));
        return Ok(0);
    }

    let parsed = bucket_from_state_uri(&coord.state_uri);
    let bucket = if parsed.is_empty() {
        config::bucket().to_string()
    } else {
        parsed
    };
    let store = JobStorage::with_bucket(&bucket)
        .await
        .map_err(|exc| exc.to_string())?;
    let interval = coord.interval_seconds.max(15) as u64;
    log(&format!(
        "coordinator '{}' runtime={} interval={interval}s state={}",
        coord.name, coord.runtime, coord.state_uri
    ));

    let secrets = secrets_from_env();
    loop {
        let mut update_log = |message: &str| log(message);
        match crate::self_update::self_update(&mut update_log).await {
            Ok(crate::self_update::UpdateOutcome::Updated { from, to }) => {
                log(&format!(
                    "coordinator self-update installed {from} -> {to}; re-executing"
                ));
                let exc = crate::self_update::reexec();
                log(&format!(
                    "coordinator self-update re-exec failed; continuing old process image: {exc}"
                ));
            }
            Ok(crate::self_update::UpdateOutcome::UpToDate { .. }) => {}
            Err(exc) => log(&format!(
                "coordinator self-update failed; continuing current version: {exc}"
            )),
        }
        // Re-resolve the coordinator entry from the registry each tick. The
        // initial resolve at process start captures the entry once and
        // never re-checks; if an operator pushes a new registry that
        // removes/renames the entry to stop a racing daemon, the running
        // process keeps reaping VMs forever using the cached entry.
        // Confirmed live 2026-05-15: a stale mac mini daemon kept deleting
        // fresh-heartbeat Llama/Qwen3 VMs for 4+ hours after the registry
        // entry was removed because pip drift never fired (the daemon was
        // already on the latest published version). Re-resolving each tick
        // means a registry change takes effect within one interval_seconds
        // without depending on a new release being published.
        // Python reads source="gcs" — the remote registry is the ONLY
        // authority for the self-survival check there, with no local
        // escape hatch. Same here, but a registry we could not READ is
        // not an authority at all.
        if let Some(target) = target {
            let survival = fetch_registry_remote().await;
            // Exit ONLY when a registry we actually READ omits the entry.
            // An unreachable store says nothing about whether the operator
            // revoked us — see `targets::RegistryFetchError`.
            if matches!(&survival, Ok(registry) if registry.lookup_coordinator(target).is_none()) {
                log(&format!(
                    "coordinator '{target}' not in the canonical registry; exiting. \
                     Operator removed/renamed the entry — daemon stops here so \
                     launchd/supervisor backs off and stale code stops issuing \
                     GCE mutations."
                ));
                return Ok(0);
            }
            if let Err(exc) = survival {
                log(&format!(
                    "canonical registry unreachable ({exc}); SKIPPING the \
                     self-survival check for coordinator '{target}' and \
                     CONTINUING. A storage outage must never mass-terminate \
                     the fleet — the kill switch fires only against a \
                     registry that was actually read."
                ));
            }
        }
        let providers = resolve_providers();
        let n = run_tick(&store, &secrets, &providers, true, &log)
            .await
            .map_err(|exc| exc.to_string())?;
        log(&format!("tick scheduled={n}"));
        if once {
            return Ok(0);
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{job_state, Job};
    use crate::providers::ProviderError;
    use crate::queue::local_file::LocalBackend;
    use crate::schedules::{read_schedule, write_schedule, Schedule};
    use std::sync::Mutex;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    /// Offline provider: a fleet of agent VMs with ages, everything else
    /// benign; delete calls are recorded in order.
    struct FakeProvider {
        refs: Vec<(String, f64)>,
        deletes: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Provider for FakeProvider {
        async fn create_instance(
            &self,
            _name: &str,
            _machine_type: &str,
            _accel_type: &str,
            _boot_disk_gb: i64,
            _image: &str,
            _image_project: &str,
            _startup_script: &str,
            _preemptible: bool,
        ) -> Result<Option<String>, ProviderError> {
            Ok(None)
        }
        async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
            self.deletes.lock().unwrap().push(instance_ref.to_string());
            Ok(())
        }
        async fn instance_exists(&self, _instance_ref: &str) -> Result<bool, ProviderError> {
            Ok(true)
        }
        async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
            Ok(BTreeMap::new())
        }
        async fn list_running_instance_refs_with_age(
            &self,
        ) -> Result<Vec<(String, f64)>, ProviderError> {
            Ok(self.refs.clone())
        }
    }

    /// One full coordinator tick over fabricated state: a due schedule, a
    /// queued job, a running job whose status blob says COMPLETED, and a
    /// dead agent VM — asserting the exact storage mutations in order.
    #[tokio::test]
    async fn full_tick_sequences_storage_mutations() {
        let (_dir, store) = store();

        // Due schedule. The command trips submit-time validation
        // (deprecated activation entrypoint) so fire_due_schedules consumes
        // the occurrence WITHOUT calling submit_via_gcs — which would build
        // a real GCS-backed store and could touch the production queue on a
        // credentialed machine. The claim mutation (advanced next_due_at)
        // is the hermetic, assertable outcome.
        let mut sched = Schedule::new(
            "sch-ticktest",
            "* * * * *",
            "python -m wisent.scripts.activations.extract_and_upload --x",
        );
        let past = crate::models::isoformat_utc(Utc::now() - chrono::Duration::minutes(5));
        sched.next_due_at = past.clone();
        write_schedule(&store, &sched).await.unwrap();

        // Queued job. Provider name "azure" has no live quota source
        // (TODO(phase-3) arm) and the store has no overlay blob, so
        // schedule_queued_jobs finds zero slots and the job stays queued —
        // deterministic and offline.
        let queued = Job::new("queuejob1", "echo hello");
        store.write_job("queue", &queued).await.unwrap();

        // Running job past boot grace whose agent wrote COMPLETED.
        let mut running = Job::new("runjob01", "echo train");
        running.state = job_state::RUNNING.to_string();
        running.instance_ref = Some("wisent-agent-x-1@zone-a".to_string());
        running.started_at = Some(crate::models::isoformat_utc(
            Utc::now() - chrono::Duration::hours(2),
        ));
        store.write_job("running", &running).await.unwrap();
        store
            .upload_text("status/runjob01/status", "COMPLETED")
            .await
            .unwrap();
        store
            .upload_text("status/runjob01/heartbeat", "RUNNING old")
            .await
            .unwrap();

        // Dead agent: past the 1800s boot grace, no capacity broadcast.
        let fake = Arc::new(FakeProvider {
            refs: vec![("wisent-agent-dead1@zone-a".to_string(), 2000.0)],
            deletes: Mutex::new(Vec::new()),
        });
        let providers = vec![ResolvedProvider::Cloud {
            name: "azure".to_string(),
            provider: fake.clone(),
        }];

        let logs: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let log = |msg: &str| logs.lock().unwrap().push(msg.to_string());
        let n = run_tick(&store, &BTreeMap::new(), &providers, false, &log)
            .await
            .unwrap();
        assert_eq!(n, 0);

        // 1. Schedule: occurrence consumed — next_due_at advanced into the
        //    future, no job submitted, fire_count untouched.
        let after = read_schedule(&store, "sch-ticktest")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(after.next_due_at, past);
        assert!(after.next_due_at > crate::models::isoformat_utc(Utc::now()));
        assert_eq!(after.fire_count, 0);

        // 2. Running job finalized: running/ -> completed/, status dir
        //    cleaned, completed_at stamped.
        let done = store
            .read_job("completed", "runjob01")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(done.state, job_state::COMPLETED);
        assert!(done.completed_at.is_some());
        assert!(store
            .read_job("running", "runjob01")
            .await
            .unwrap()
            .is_none());
        assert!(store
            .download_text("status/runjob01/status")
            .await
            .unwrap()
            .is_none());
        assert!(store
            .download_text("status/runjob01/heartbeat")
            .await
            .unwrap()
            .is_none());

        // 3. Provider mutations, in tick order: the completed job's VM
        //    first (check_running_jobs), then the dead agent (reaper).
        let deletes = fake.deletes.lock().unwrap().clone();
        assert_eq!(
            deletes,
            vec![
                "wisent-agent-x-1@zone-a".to_string(),
                "wisent-agent-dead1@zone-a".to_string(),
            ]
        );

        // 4. Queued job untouched (no quota -> no dispatch).
        assert!(store
            .read_job("queue", "queuejob1")
            .await
            .unwrap()
            .is_some());

        // 5. Coordinator log lines.
        let logs = logs.lock().unwrap();
        assert!(logs.iter().any(|m| m == "azure: reaped 1 dead-agent VM(s)"));
    }
}
