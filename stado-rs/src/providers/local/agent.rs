//! Local GPU agent main loop: polls the queue, runs jobs in concurrent
//! slots, respects Vast.ai renters, broadcasts capacity, and cooperatively
//! yields lower-priority slots for higher-priority queued work.
//!
//! The runtime contract is framework-neutral:
//!   * no Python package is imported before claim;
//!   * NVIDIA admission uses the native `nvidia-smi` driver interface;
//!   * optional Hugging Face staging runs only when both
//!     `STADO_HF_FLUSH_STAGING_DIR` and `STADO_HF_FLUSH_PYTHON` are set;
//!   * job-specific runtimes, libraries, and GPU framework checks belong to
//!     the submitted workload.
//!
//! Registry self-lookup uses the configured Stado storage backend with the
//! bundled registry only as the documented fallback. Local release drift
//! triggers exact-coordinate binary self-update and re-exec; cloud machines
//! self-terminate for provider-owned replacement. A registry GPU-type change
//! remains an explicit operator restart.
//!
//! The janitor-owned disk-cleanup engine IS ported ([`super::disk_cleanup`]):
//! this loop runs `run_cleanup_once` every tick, holds a shared workload
//! lock per live slot, and publishes zero capacity while disk pressure is
//! unresolved. One behavioral simplification: Python releases a finished
//! slot's workload lock explicitly and logs release failures; the Rust
//! port lets the [`ActiveSlot`]'s `Drop` close the lock file (flock is
//! released on close), which cannot fail in a way worth logging.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Map, Value};

use crate::config::estimate_gpu_memory;
use crate::constants;
use crate::models::{activation_extraction_must_share_gpu, isoformat_utc};
use crate::queue::capacity::publish_capacity;
use crate::queue::control;
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;
use crate::targets::{self, ComputeTarget, Registry, RegistryError};

use super::disk_cleanup;
use super::disk_gate::{self, DiskGateDiag};
use super::fleet_flush::spawn_fleet_flush;
use super::helpers;
use super::self_terminate;
use super::slots::{advance_slot, request_yield, ActiveSlot, SlotOutcome, DEFAULT_MAX_YIELDS};
use super::version_check::{self, DriftOutcome};

/// Main agent poll interval (latency vs. storage-API load trade-off).
pub const POLL_INTERVAL_S: u64 = constants::POLL_INTERVAL_S;

/// Cooperative-yield anti-thrash floor: never evict a yieldable slot that has
/// run for less than this, so a just-(re)started background job gets real work
/// done before it can be bumped again. Pairs with Job.max_yields_before_protected.
pub const MIN_RUNTIME_BEFORE_YIELD_S: u64 = constants::MIN_RUNTIME_BEFORE_YIELD_S;

/// Cache TTL for the native NVIDIA driver-health probe.
pub const CUDA_PROBE_CACHE_S: u64 = constants::CUDA_PROBE_CACHE_S;

/// Python `_log`: `[HH:MM:SS] [agent] msg` on stderr (local time).
pub fn agent_log(msg: &str) {
    let ts = chrono::Local::now().format("%H:%M:%S");
    eprintln!("[{ts}] [agent] {msg}");
}

/// Hard VRAM safety buffer at admission. The agent refuses to claim a
/// job if accepting it would leave less than this margin between
/// declared total VRAM use and the GPU's physical capacity. Catches the
/// class of failure where neighbor processes' actual peak exceeds their
/// declared gpu_mem_gb (estimate_gpu_memory has been observed to
/// under-call by 5-10 GB on 7-8B activation extraction workloads). The
/// buffer is independent of the per-job multipliers because it's the
/// LAST line of defense — if the per-job estimate is wrong, this catches
/// it before the n+1th job OOMs the entire VM.
/// Derived from total VRAM instead of a flat constant.
/// Python `_vram_safety_buffer_gb`.
pub fn vram_safety_buffer_gb(total_vram_gb: i64) -> i64 {
    (constants::VRAM_SAFETY_BUFFER_MIN_GB as i64)
        .max((total_vram_gb as f64 * constants::VRAM_SAFETY_BUFFER_FRACTION).ceil() as i64)
}

/// Python `int(os.environ.get(key, "0") or 0)`: unset or empty -> default;
/// a non-integer value panics (Python's ValueError crashes the agent).
fn env_i64(key: &str, default: i64) -> i64 {
    match std::env::var(key) {
        Ok(raw) if !raw.is_empty() => raw
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{key} must be an int (Python int() parity): {raw}")),
        _ => default,
    }
}

/// Python `float(os.environ.get(key, default) or default)`.
fn env_f64(key: &str, default: f64) -> f64 {
    match std::env::var(key) {
        Ok(raw) if !raw.is_empty() => raw
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{key} must be a float (Python float() parity): {raw}")),
        _ => default,
    }
}

/// Python `float(os.environ.get(primary, os.environ.get(fallback, d)) or d)`.
fn env_f64_chain(primary: &str, fallback: &str, default: f64) -> f64 {
    let parse = |key: &str, raw: String| {
        raw.trim()
            .parse()
            .unwrap_or_else(|_| panic!("{key} must be a float (Python float() parity): {raw}"))
    };
    match std::env::var(primary) {
        Ok(raw) if !raw.is_empty() => parse(primary, raw),
        Ok(_) => default,
        Err(_) => match std::env::var(fallback) {
            Ok(raw) if !raw.is_empty() => parse(fallback, raw),
            _ => default,
        },
    }
}

// ---------------------------------------------------------------------------
// registry self-lookup (Python targets.load_targets(source="auto"))
// ---------------------------------------------------------------------------

/// The registry document, GCS first with the bundled file as fallback
/// (Python `load_targets(source="auto")`). The fetch + 30 s TTL cache live
/// in [`targets`] — see `targets::fetch_registry_remote`.
pub async fn load_registry_auto() -> Result<Registry, RegistryError> {
    targets::load_registry_auto().await
}

/// Find the unique target declaring this host's identity.
/// Python `targets.lookup_self(hostname, source="auto")`.
pub async fn lookup_self_auto(hostname: &str) -> Result<Option<ComputeTarget>, RegistryError> {
    let registry = load_registry_auto().await?;
    Ok(registry.lookup_self(hostname)?.cloned())
}

/// Return the named target (Python `targets.lookup(name, source="auto")`).
pub async fn lookup_auto(name: &str) -> Result<Option<ComputeTarget>, RegistryError> {
    let registry = load_registry_auto().await?;
    Ok(registry.lookup(name).cloned())
}

// ---------------------------------------------------------------------------
// CUDA child probe
// ---------------------------------------------------------------------------

struct CudaProbe {
    checked_at: Instant,
    ok: bool,
    detail: String,
}

static CUDA_PROBE: LazyLock<Mutex<Option<CudaProbe>>> = LazyLock::new(|| Mutex::new(None));

/// Check that the host's native NVIDIA management interface can enumerate a
/// GPU. Workload-specific CUDA frameworks are validated by the workload
/// itself; the global agent must not import Python or `wisent` before claiming
/// an unrelated shell, native, or container job.
pub async fn gpu_driver_available() -> (bool, String) {
    if let Some(probe) = &*CUDA_PROBE.lock().expect("cuda probe cache lock") {
        if probe.checked_at.elapsed() < Duration::from_secs(CUDA_PROBE_CACHE_S) {
            return (probe.ok, probe.detail.clone());
        }
    }
    let (ok, detail) = run_cuda_probe().await;
    *CUDA_PROBE.lock().expect("cuda probe cache lock") = Some(CudaProbe {
        checked_at: Instant::now(),
        ok,
        detail: detail.clone(),
    });
    (ok, detail)
}

async fn run_cuda_probe() -> (bool, String) {
    let res = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=uuid", "--format=csv,noheader,nounits"])
            .output(),
    )
    .await;
    match res {
        Ok(Ok(out)) => cuda_probe_result(
            out.status.code().unwrap_or(-1),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Ok(Err(exc)) => (false, format!("cuda probe raised: {exc}")),
        Err(_) => (false, "cuda probe raised: timed out after 30s".to_string()),
    }
}

/// Pure: `(ok, detail)` from one native driver probe. Detail is truncated to
/// a bounded suffix for capacity diagnostics.
pub fn cuda_probe_result(rc: i32, stdout: &str, stderr: &str) -> (bool, String) {
    let raw = if !stdout.is_empty() {
        stdout.to_string()
    } else if !stderr.is_empty() {
        stderr.to_string()
    } else {
        format!("rc={rc}")
    };
    let trimmed = raw.trim();
    let detail: String = trimmed
        .chars()
        .rev()
        .take(300)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    (rc == 0, detail)
}

// ---------------------------------------------------------------------------
// cooperative priority yield
// ---------------------------------------------------------------------------

/// The yield-relevant facts of one running slot, extracted so the eviction
/// choice is a pure function (Python reads these off the slot dict + Job).
#[derive(Debug, Clone)]
pub struct YieldSlotInfo {
    pub job_id: String,
    pub priority: i64,
    pub yieldable: bool,
    /// `_slot_is_exclusive(slot)` — exclusive slots are never evicted.
    pub exclusive: bool,
    pub yield_count: i64,
    pub max_yields_before_protected: i64,
    pub started_mono: Instant,
    /// `_slot_vram(slot)` — best known VRAM footprint.
    pub vram_gb: i64,
}

/// Choose which slots to yield so `need` GB fits. Pure: the eviction half
/// of Python `_maybe_yield_for_priority`.
///
/// Evictable = yieldable, non-exclusive, strictly lower priority than the
/// target, not yet yield-protected, and past the anti-thrash runtime floor.
/// Evict lowest-priority first; among equal priority, free the largest slot
/// first so we yield as few jobs as possible. Returns empty when even
/// yielding every candidate won't fit — don't waste a yield.
pub fn choose_yield_slots(
    slots: &[YieldSlotInfo],
    target_prio: i64,
    need: i64,
    free_vram_gb: i64,
    now: Instant,
) -> Vec<usize> {
    let mut evictable: Vec<usize> = (0..slots.len())
        .filter(|&i| {
            let s = &slots[i];
            // Python `int(getattr(job, "max_yields_before_protected", 5) or 5)`:
            // a stored 0 falls back to 5.
            let max_yields = if s.max_yields_before_protected != 0 {
                s.max_yields_before_protected
            } else {
                DEFAULT_MAX_YIELDS
            };
            s.yieldable
                && !s.exclusive
                && s.priority < target_prio
                && s.yield_count < max_yields
                && now.saturating_duration_since(s.started_mono)
                    >= Duration::from_secs(MIN_RUNTIME_BEFORE_YIELD_S)
        })
        .collect();
    if evictable.is_empty() {
        return Vec::new();
    }
    evictable.sort_by_key(|&i| (slots[i].priority, -slots[i].vram_gb));
    let mut freed = 0i64;
    let mut chosen = Vec::new();
    for i in evictable {
        chosen.push(i);
        freed += slots[i].vram_gb;
        if free_vram_gb + freed >= need {
            break;
        }
    }
    if free_vram_gb + freed < need {
        return Vec::new();
    }
    chosen
}

/// If a strictly-higher-priority eligible queued job can't fit in the
/// current free VRAM, cooperatively yield just enough lower-priority
/// yieldable slots to make room. Returns the number of slots yielded
/// (removed from `slots`); 0 means no action.
/// Python `_maybe_yield_for_priority`.
///
/// Inert by construction: returns immediately unless a yieldable job is
/// actually running, so existing (non-yieldable) prod workloads never enter
/// the queue scan or any eviction logic.
#[allow(clippy::too_many_arguments)]
pub async fn maybe_yield_for_priority(
    store: &JobStorage,
    sizing: &Sizing,
    slots: &mut Vec<ActiveSlot>,
    gpu_type: &str,
    total_vram_gb: i64,
    free_vram_gb: i64,
    kind: &str,
    consumer_id: &str,
    log_fn: &mut dyn FnMut(&str),
) -> Result<usize, StorageError> {
    if !slots.iter().any(|s| s.slot.job.yieldable) {
        return Ok(0);
    }
    // Highest-priority queued job that needs MORE than current free VRAM but
    // could fit on the full GPU, and is eligible for THIS agent.
    let mut candidates = store.list_jobs_fitting("queue", total_vram_gb, 200).await?;
    candidates.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    let mut target: Option<(crate::models::Job, i64)> = None;
    for j in &candidates {
        let need_j = j
            .gpu_mem_gb
            .max(estimate_gpu_memory(&j.command, sizing, store).await?);
        if need_j <= free_vram_gb {
            continue; // already fits — not a VRAM-eviction case
        }
        if !helpers::job_eligible(
            j,
            gpu_type,
            total_vram_gb,
            kind,
            consumer_id,
            slots.len(),
            false,
        ) {
            continue;
        }
        target = Some((j.clone(), need_j));
        break;
    }
    let Some((target, need)) = target else {
        return Ok(0);
    };
    let target_prio = target.priority;

    let now = Instant::now();
    let mut infos = Vec::with_capacity(slots.len());
    for s in slots.iter() {
        infos.push(YieldSlotInfo {
            job_id: s.slot.job.job_id.clone(),
            priority: s.slot.job.priority,
            yieldable: s.slot.job.yieldable,
            exclusive: helpers::slot_is_exclusive(&s.slot),
            yield_count: s.slot.job.yield_count,
            max_yields_before_protected: s.slot.job.max_yields_before_protected,
            started_mono: s.started_mono,
            vram_gb: helpers::slot_vram(&s.slot, sizing, store).await?,
        });
    }
    let chosen = choose_yield_slots(&infos, target_prio, need, free_vram_gb, now);
    if chosen.is_empty() {
        return Ok(0);
    }
    let freed: i64 = chosen.iter().map(|&i| infos[i].vram_gb).sum();
    let mut n = 0usize;
    // Remove in descending index order so earlier removals don't shift the
    // indices of later ones (Python `slots.remove(s)` on identity).
    for &idx in chosen.iter().rev() {
        let s = slots.remove(idx);
        let jid = s.slot.job.job_id.clone();
        match request_yield(s, store, log_fn).await {
            Ok(true) => n += 1,
            Ok(false) => {}
            Err(exc) => log_fn(&format!("yield: request_yield raised for {jid}: {exc}")),
        }
    }
    if n > 0 {
        log_fn(&format!(
            "yield: freed ~{freed}G via {n} slot(s) for higher-priority {} (need={need}G prio={target_prio})",
            target.job_id
        ));
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// run_agent
// ---------------------------------------------------------------------------

/// The previous tick's capacity snapshot, republished at the top of the
/// next tick as a keep-alive (Python `_last_cap`).
struct LastCap {
    free_slots: BTreeMap<String, i64>,
    free_vram_gb: i64,
    total_vram_gb: i64,
    diag: Map<String, Value>,
}

fn diag_map(d: &DiskGateDiag) -> [(String, Value); 4] {
    [
        ("free_disk_gb".to_string(), Value::from(d.free_disk_gb)),
        (
            "home_write_probe_ok".to_string(),
            Value::from(d.home_write_probe_ok),
        ),
        (
            "staging_free_gb".to_string(),
            Value::from(d.staging_free_gb),
        ),
        (
            "largest_pending_raw_dir_gb".to_string(),
            Value::from(d.largest_pending_raw_dir_gb),
        ),
    ]
}

/// Main agent loop. Polls queue, runs jobs when Vast.ai is idle.
/// Python `run_agent`.
///
/// idle_shutdown=true: exit cleanly once both: (a) no slots are active and
/// (b) no queued job is eligible for this consumer's free VRAM. The scheduler
/// observes the stopped capacity heartbeat and releases ephemeral machines
/// through their owning provider adapters.
///
/// kind: capacity-broadcast label distinguishing physical workstations
/// (kind="local") from ephemeral cloud-agent VMs (kind="gcp", ...).
/// No global error handler wraps the loop body: unexpected errors
/// crash the agent visibly (returned as Err) so the operator can diagnose.
pub async fn run_agent(gpu_type: &str, idle_shutdown: bool, kind: &str) -> anyhow::Result<()> {
    let log_fn = &mut |msg: &str| agent_log(msg);

    let mut gpu_type = gpu_type.to_string();
    if gpu_type.is_empty() {
        gpu_type = helpers::detect_gpu_type().await;
    }
    let mut total_vram_gb = helpers::detect_local_vram_gb().await.max(1);
    let hard_slot_cap = env_i64("WC_LOCAL_SLOTS", 0);
    // No default cap: local admission is governed by live VRAM/RAM/disk gates.
    log_fn(&format!(
        "Agent started. kind={kind}  GPU: {gpu_type}  vram_gb={total_vram_gb}  hard_slot_cap={hard_slot_cap}"
    ));
    super::disk_staging::setup_agent_staging(log_fn).await;

    let hostname = crate::providers::vast::system_hostname();
    log_fn("init: legacy workdir reaping disabled; cleanup is policy-owned");

    let initial_gpu = gpu_type.clone();

    let store = JobStorage::new().await?;
    log_fn("init: JobStorage done");
    let sizing = Sizing::new();
    let consumer_id = format!("{kind}-{hostname}");
    let mut slots: Vec<ActiveSlot> = Vec::new();
    let mut agent_diag: Map<String, Value> = Map::new();
    let fleet_staging = std::env::var("STADO_HF_FLUSH_STAGING_DIR")
        .ok()
        .filter(|path| !path.trim().is_empty());
    let mut last_fleet_flush = Instant::now();

    let mut last_cap: Option<LastCap> = None;
    let mut pinned_only = false; // registry ComputeTarget.pinned_only, refreshed per poll
                                 // Python `disk_low_bytes = _persisted_disk_low_bytes()`: reuse the last
                                 // canonical low watermark from the janitor's owner-controlled state
                                 // file (cleanup may be unable to reach the registry during startup).
    let mut disk_low_bytes = disk_cleanup::persisted_disk_low_bytes();
    if disk_low_bytes.is_some() {
        log_fn("init: loaded validated disk low watermark from janitor state");
    }
    loop {
        // Phase breadcrumbs for the 40GB a2-highgpu-1g first-iter hang.
        log_fn("loop: iter-start");
        if let Err(exc) = crate::config::refresh_model_policy(&store).await {
            log_fn(&format!(
                "model policy refresh failed; retaining last good policy: {exc}"
            ));
        }
        // DEVIATION: the wisent upload_worker sweep is not ported (the
        // wisent Python package owns it); the fleet-flush subprocess path
        // below covers the same pending pool.
        let vast_active = if crate::config::wc_providers()
            .iter()
            .any(|provider| provider == crate::capabilities::ProviderId::Vast.as_str())
            && !crate::config::wc_disabled_providers()
                .iter()
                .any(|provider| provider == crate::capabilities::ProviderId::Vast.as_str())
        {
            helpers::vast_has_renter().await?
        } else {
            false
        };
        let mut survivors: Vec<ActiveSlot> = Vec::with_capacity(slots.len());
        for slot in slots.drain(..) {
            match advance_slot(slot, &store, &sizing, vast_active, log_fn).await? {
                SlotOutcome::Running(slot) => survivors.push(slot),
                SlotOutcome::Done => {}
            }
        }
        slots = survivors;
        // The janitor's bounded cleanup pass (Python `run_cleanup_once`,
        // wrapped there in try/except BaseException — the Rust port folds
        // every failure into the returned report by construction).
        let cleanup_report =
            disk_cleanup::run_cleanup_once(slots.len() as i64, false, log_fn).await;
        agent_diag.insert("disk_cleanup".into(), cleanup_report.clone());
        if let Some(reported_low) = disk_cleanup::validated_report_low_bytes(&cleanup_report) {
            disk_low_bytes = Some(reported_low);
        }
        // Python: shutil.disk_usage(expanduser("~")).free, OSError -> None.
        let current_free_bytes =
            disk_cleanup::free_bytes(&crate::config_file::expand_tilde("~")).ok();
        let disk_policy_known = disk_low_bytes.is_some();
        let pressure_unresolved =
            disk_cleanup::disk_pressure_unresolved(disk_low_bytes, current_free_bytes);
        agent_diag.insert(
            "disk_cleanup_policy_known".into(),
            Value::from(disk_policy_known),
        );
        agent_diag.insert(
            "disk_pressure_unresolved".into(),
            Value::from(pressure_unresolved),
        );
        if pressure_unresolved {
            // Fail admission closed until both the policy threshold and
            // free space are known (Python's zero-cap gate).
            let zero_diag = agent_diag.clone();
            let _ = publish_capacity(
                &store,
                &consumer_id,
                kind,
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(zero_diag.clone()),
            )
            .await;
            last_cap = Some(LastCap {
                free_slots: BTreeMap::new(),
                free_vram_gb: 0,
                total_vram_gb,
                diag: zero_diag,
            });
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
            continue;
        }
        // The republish keep-alive below is unchanged from Python.
        if let Some(cap) = &last_cap {
            let _ = publish_capacity(
                &store,
                &consumer_id,
                kind,
                &cap.free_slots,
                Some(cap.free_vram_gb),
                Some(cap.total_vram_gb),
                Some(cap.diag.clone()),
            )
            .await;
        }
        if last_fleet_flush.elapsed() > Duration::from_secs(constants::FLEET_FLUSH_INTERVAL_S)
            && slots.is_empty()
        {
            if let Some(fleet_staging) = fleet_staging.as_deref() {
                if spawn_fleet_flush(Path::new(fleet_staging), log_fn).await? {
                    log_fn("optional Hugging Face staging flush running asynchronously");
                }
            }
            last_fleet_flush = Instant::now();
        }
        if let Some(t) = lookup_self_auto(&hostname).await? {
            if t.is_provider(crate::capabilities::ProviderId::Local) {
                // Env overrides are now owned by systemd (/etc/wisent/wisent-agent.env).
                // Ignore registry env deltas so an external registry push cannot
                // trigger a pip reinstall loop or override local tuning.
                if let Some(t_gpu) = &t.gpu_type {
                    if !t_gpu.is_empty() && *t_gpu != initial_gpu && slots.is_empty() {
                        log_fn(&format!(
                            "Registry gpu_type {initial_gpu} -> {t_gpu}; pip_upgrade_and_exec for restart"
                        ));
                        // DEVIATION: no re-exec here — the self-update
                        // path fires on version drift only (see
                        // version_check); a gpu_type change remains an
                        // operator-restart action.
                    }
                }
                if let Some(t_vram) = t.vram_gb {
                    if t_vram > 0 && t_vram != total_vram_gb {
                        log_fn(&format!(
                            "Registry vram_gb override {total_vram_gb} -> {t_vram}"
                        ));
                        total_vram_gb = t_vram;
                    }
                }
                pinned_only = t.pinned_only;
                if pinned_only {
                    agent_diag.insert("pinned_only".into(), Value::from(true));
                }
            }
        }
        // Cleanup already ran before the immutable release check. This gate is
        // admission/diagnostics-only and has no destructive side effects.
        let (_pre_refuse, pre_diag) = disk_gate::gate_and_maybe_evict(log_fn);
        agent_diag.extend(diag_map(&pre_diag));
        log_fn("loop: pre-drain release drift check");
        match version_check::maybe_drain_or_upgrade(!slots.is_empty(), log_fn, kind).await {
            // An update/re-exec failure was logged; keep claiming on the old
            // binary rather than wedging the fleet. A successful update
            // re-execs and never reaches this arm.
            DriftOutcome::Clean | DriftOutcome::DriftDetected => {}
            // Cloud replacement was requested through the provider adapter.
            DriftOutcome::SelfTerminated => return Ok(()),
        }
        let inference_reservation = crate::inference::reservation::active();
        if vast_active {
            publish_capacity(
                &store,
                &consumer_id,
                kind,
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(agent_diag.clone()),
            )
            .await?;
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
            continue;
        }

        let mut used_vram = 0i64;
        for s in &slots {
            used_vram += helpers::slot_vram(&s.slot, &sizing, &store).await?;
        }
        if slots.iter().any(|s| helpers::slot_is_exclusive(&s.slot)) {
            used_vram = total_vram_gb;
        }
        let mut free_vram_gb = (total_vram_gb - used_vram).max(0);
        let smi_free = helpers::smi_free_vram_gb().await;
        if smi_free >= 0 && smi_free < free_vram_gb {
            free_vram_gb = smi_free;
        }
        if let Some(reservation) = &inference_reservation {
            agent_diag.insert(
                "inference_reservation".into(),
                Value::from(reservation.deployment.clone()),
            );
            free_vram_gb = 0;
            log_fn(&format!(
                "exclusive inference reservation '{}': GPU claims disabled; CPU-only claims remain eligible",
                reservation.deployment
            ));
        }
        let (refuse_disk, disk_diag) = disk_gate::gate_and_maybe_evict(log_fn);
        agent_diag.extend(diag_map(&disk_diag));
        if refuse_disk {
            publish_capacity(
                &store,
                &consumer_id,
                kind,
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(agent_diag.clone()),
            )
            .await?;
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }
        let vram_buffer_gb = vram_safety_buffer_gb(total_vram_gb);
        let mut settling_ids: Vec<String> = Vec::new();
        for s in &slots {
            if helpers::slot_waiting_for_vram(&s.slot, &sizing, &store).await? {
                settling_ids.push(s.slot.job.job_id.clone());
            }
        }
        if !settling_ids.is_empty() {
            agent_diag.insert(
                "settling_slot_ids".into(),
                Value::Array(settling_ids.into_iter().map(Value::String).collect()),
            );
            publish_capacity(
                &store,
                &consumer_id,
                kind,
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(agent_diag.clone()),
            )
            .await?;
            last_cap = Some(LastCap {
                free_slots: BTreeMap::new(),
                free_vram_gb: 0,
                total_vram_gb,
                diag: agent_diag.clone(),
            });
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
            continue;
        }
        if free_vram_gb < vram_buffer_gb {
            // VRAM-tight host (apple-mps reports ~1GB): broadcast zero VRAM
            // capacity so the coordinator routes no VRAM work here, but keep
            // scanning the queue — jobs with need==0 (CPU-only: probierz
            // runs, smoke checks) stay claimable. The per-job VRAM checks
            // below still reject anything needing VRAM we don't have.
            agent_diag.insert("vram_buffer_gb".into(), Value::from(vram_buffer_gb));
            agent_diag.insert("vram_buffer_free_gb".into(), Value::from(free_vram_gb));
            publish_capacity(
                &store,
                &consumer_id,
                kind,
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(agent_diag.clone()),
            )
            .await?;
            // Python also stamps _last_cap here, but the normal publish
            // below overwrites it in the same iteration on every path
            // (the cuda-fail continue stamps its own) — dead in both.
        }
        if free_vram_gb > 0 && slots.is_empty() && gpu_type.starts_with("nvidia") {
            let (cuda_ok, cuda_detail) = gpu_driver_available().await;
            agent_diag.insert("gpu_driver_ok".into(), Value::from(cuda_ok));
            agent_diag.insert("gpu_driver_detail".into(), Value::from(cuda_detail.clone()));
            agent_diag.insert(
                "gpu_driver_checked_at".into(),
                Value::from(isoformat_utc(Utc::now())),
            );
            if !cuda_ok {
                log_fn(&format!(
                    "NVIDIA driver probe failed; publishing zero capacity: {}",
                    cuda_detail.chars().take(160).collect::<String>()
                ));
                publish_capacity(
                    &store,
                    &consumer_id,
                    kind,
                    &BTreeMap::new(),
                    Some(0),
                    Some(total_vram_gb),
                    Some(agent_diag.clone()),
                )
                .await?;
                last_cap = Some(LastCap {
                    free_slots: BTreeMap::new(),
                    free_vram_gb: 0,
                    total_vram_gb,
                    diag: agent_diag.clone(),
                });
                tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
                continue;
            }
        }
        let free_slots = helpers::build_capacity_dict(&gpu_type, free_vram_gb, total_vram_gb);
        publish_capacity(
            &store,
            &consumer_id,
            kind,
            &free_slots,
            Some(free_vram_gb),
            Some(total_vram_gb),
            Some(agent_diag.clone()),
        )
        .await?;
        last_cap = Some(LastCap {
            free_slots: free_slots.clone(),
            free_vram_gb,
            total_vram_gb,
            diag: agent_diag.clone(),
        });

        // Maintenance-mode gate (queue::control — read that module for the
        // full semantics). A paused queue means this agent starts NOTHING
        // new, so the entire admission half of the tick is skipped: no
        // cooperative yield (evicting a running job to free VRAM for a
        // claim that can never happen would destroy work for nothing), no
        // queue listing, no claim. Everything above this point has already
        // run — advance_slot drove every live slot, heartbeats went out,
        // and capacity was published — so jobs already in running/ finish
        // normally and `stado queue drain --wait` can terminate.
        //
        // Re-read every iteration, never cached: `stado queue resume` has
        // to reach a running agent without an operator restarting it.
        let queue_control = control::read(&store).await?;
        agent_diag.insert("queue_paused".into(), Value::from(queue_control.paused));
        if queue_control.paused {
            agent_diag.insert(
                "queue_pause_reason".into(),
                Value::from(queue_control.pause_summary()),
            );
            log_fn(&format!(
                "Queue paused ({}); claiming nothing",
                queue_control.pause_summary()
            ));
            // An ephemeral cloud agent with no slots left is idle while paused.
            // Exit so the capacity heartbeat stops; dispatch is paused, and the
            // owning provider adapter reaps the machine without replacing it.
            if idle_shutdown && slots.is_empty() {
                log_fn("idle_shutdown: no slots + queue paused; exiting");
                self_terminate(kind, log_fn).await;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
            continue;
        }

        // Cooperative yield: if a higher-priority queued job can't fit, evict
        // just enough lower-priority yieldable slots to make room. Runs BEFORE
        // the full-GPU early-return below because that is exactly when it's
        // needed. Inert (single any() over slots) unless a yieldable job runs.
        if maybe_yield_for_priority(
            &store,
            &sizing,
            &mut slots,
            &gpu_type,
            total_vram_gb,
            free_vram_gb,
            kind,
            &consumer_id,
            log_fn,
        )
        .await?
            > 0
        {
            continue; // re-loop: recompute free VRAM, then claim the freed room
        }

        let all_active_share_gpu = |slots: &[ActiveSlot]| {
            slots
                .iter()
                .all(|s| activation_extraction_must_share_gpu(&s.slot.job.command))
        };
        let slot_cap_reached = hard_slot_cap > 0 && slots.len() as i64 >= hard_slot_cap;
        if slot_cap_reached && !all_active_share_gpu(&slots) {
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }
        // RAM gate: ordinary compute retains the measured non-wisent reserve.
        // An exclusive inference reservation already reduces the scan to
        // zero-VRAM jobs; for those, MemAvailable is the usable headroom and
        // only the dynamic safety buffer is reserved. Counting the inference
        // process RSS again would double-count memory already excluded from
        // MemAvailable and make every CPU-only maintenance job impossible.
        let fr = helpers::free_ram_gb();
        let ram_reserve = if inference_reservation.is_some() {
            helpers::ram_safety_buffer_gb()
        } else {
            helpers::static_ram_reserve_gb() + helpers::ram_safety_buffer_gb()
        };
        if (0.0..ram_reserve).contains(&fr) {
            log_fn(&format!(
                "RAM gate: {} GB free < {} GB reserve; skipping claims",
                fr as i64, ram_reserve as i64
            ));
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }

        // Centralized assignment writes job.assigned_to on the queue blob;
        // job_eligible(consumer_id=...) below filters to ONLY the jobs this
        // agent owns. The coordinator's makespan matcher already made the
        // choice; this loop executes it.
        let mut queued = store.list_jobs_fitting("queue", free_vram_gb, 2000).await?;
        queued.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        let mut started = 0i64;
        let mut diag_vram_rejected = 0i64;
        let mut diag_eligibility_rejected = 0i64;
        let mut diag_eligible = 0i64;
        let max_claims = env_i64("WC_LOCAL_MAX_CLAIMS_PER_TICK", 0);
        let raw_reserve = env_f64("WISENT_RAW_CLAIM_RESERVE_GB", 180.0);
        let raw_min_free = env_f64_chain(
            "WISENT_RAW_CLAIM_MIN_FREE_GB",
            "WISENT_RAW_HOT_FREE_TARGET_GB",
            270.0,
        );
        let tmpdir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
        let raw_root = Path::new(&tmpdir).join("wisent_raw_pending");
        let raw_free = disk_gate::free_gb(&raw_root);
        let mut raw_reserved = raw_reserve
            * slots
                .iter()
                .filter(|s| activation_extraction_must_share_gpu(&s.slot.job.command))
                .count() as f64;
        let mut diag_raw_disk_rejected = 0i64;
        for job in queued.iter() {
            let cmd = job.command.clone();
            let is_raw_share = activation_extraction_must_share_gpu(&cmd);
            if is_raw_share
                && raw_free >= 0.0
                && raw_free - raw_reserved - raw_reserve < raw_min_free
            {
                diag_raw_disk_rejected += 1;
                agent_diag.insert(
                    "raw_claim_free_gb".into(),
                    Value::from((raw_free * 10.0).round() / 10.0),
                );
                agent_diag.insert(
                    "raw_claim_reserved_gb".into(),
                    Value::from((raw_reserved * 10.0).round() / 10.0),
                );
                continue;
            }
            let share_now = all_active_share_gpu(&slots);
            let cap_reached = hard_slot_cap > 0 && slots.len() as i64 >= hard_slot_cap;
            if cap_reached && !(share_now && is_raw_share) {
                continue;
            }
            let need = job
                .gpu_mem_gb
                .max(estimate_gpu_memory(&cmd, &sizing, &store).await?);
            // Hard VRAM safety buffer: refuse if declared use after admission
            // would leave less than the dynamic VRAM safety buffer. Use live
            // free VRAM, not only slot-declared usage, so external users such
            // as ComfyUI are included in the post-claim margin.
            let claimable_vram_gb = (free_vram_gb - vram_safety_buffer_gb(total_vram_gb)).max(0);
            if need > claimable_vram_gb {
                diag_vram_rejected += 1;
                agent_diag.insert(
                    "last_buffer_reject_job_id".into(),
                    Value::from(job.job_id.clone()),
                );
                agent_diag.insert(
                    "last_buffer_reject_at".into(),
                    Value::from(isoformat_utc(Utc::now())),
                );
                agent_diag.insert("last_buffer_reject_need_gb".into(), Value::from(need));
                agent_diag.insert(
                    "last_buffer_reject_claimable_gb".into(),
                    Value::from(claimable_vram_gb),
                );
                continue;
            }
            // Also retain the slot-declared projection as a backstop for
            // cases where nvidia-smi temporarily under-reports a starting
            // child process. Only meaningful when the job actually needs
            // VRAM: on sub-buffer hosts total-buffer goes negative, which
            // would otherwise reject even need==0 (CPU-only) jobs.
            let mut projected_used = need;
            for s in &slots {
                projected_used += helpers::slot_vram(&s.slot, &sizing, &store).await?;
            }
            if need > 0 && projected_used > total_vram_gb - vram_safety_buffer_gb(total_vram_gb) {
                diag_vram_rejected += 1;
                agent_diag.insert(
                    "last_buffer_reject_job_id".into(),
                    Value::from(job.job_id.clone()),
                );
                agent_diag.insert(
                    "last_buffer_reject_at".into(),
                    Value::from(isoformat_utc(Utc::now())),
                );
                continue;
            }
            if !helpers::job_eligible(
                job,
                &gpu_type,
                total_vram_gb,
                kind,
                &consumer_id,
                slots.len(),
                pinned_only,
            ) {
                diag_eligibility_rejected += 1;
                continue;
            }
            diag_eligible += 1;
            // The disk-cleanup workload lock (Python
            // `acquire_workload_lock`): a shared hold on the janitor's
            // lock file for as long as the workload owns its slot.
            let workload_lock = match disk_cleanup::acquire_workload_lock() {
                Ok(lock) => lock,
                Err(exc) => {
                    log_fn(&format!(
                        "disk cleanup workload lock unavailable: {}",
                        exc.code
                    ));
                    agent_diag.insert("disk_cleanup_admission".into(), Value::from("lock_error"));
                    break;
                }
            };
            let Some(workload_lock) = workload_lock else {
                agent_diag.insert(
                    "disk_cleanup_admission".into(),
                    Value::from("cleanup_in_progress"),
                );
                break;
            };
            let new_slot = match super::slots::start_slot(
                &store,
                job.clone(),
                &hostname,
                log_fn,
                kind,
            )
            .await
            {
                Ok(slot) => slot,
                Err(exc) => {
                    disk_cleanup::release_workload_lock(workload_lock, log_fn);
                    return Err(exc.into());
                }
            };
            let Some(mut new_slot) = new_slot else {
                // Admission failed before spawn; do not retain a workload lock.
                disk_cleanup::release_workload_lock(workload_lock, log_fn);
                continue;
            };
            new_slot.disk_cleanup_lock = Some(workload_lock);
            slots.push(new_slot);
            free_vram_gb -= need;
            if is_raw_share {
                raw_reserved += raw_reserve;
            }
            started += 1;
            agent_diag.insert(
                "last_started_job_id".into(),
                Value::from(job.job_id.clone()),
            );
            agent_diag.insert(
                "last_started_at".into(),
                Value::from(isoformat_utc(Utc::now())),
            );
            if !is_raw_share {
                break;
            }
            if max_claims > 0 && started >= max_claims {
                break;
            }
            // DEVIATION: Python references a bare VRAM_SAFETY_BUFFER_GB
            // that does not exist in local_agent.py (latent NameError on
            // this raw multi-claim path). The computed dynamic buffer is
            // the obviously intended value.
            if free_vram_gb <= vram_buffer_gb {
                break;
            }
        }
        agent_diag.insert("queue_scanned".into(), Value::from(queued.len() as i64));
        agent_diag.insert("vram_rejected".into(), Value::from(diag_vram_rejected));
        agent_diag.insert(
            "raw_disk_rejected".into(),
            Value::from(diag_raw_disk_rejected),
        );
        agent_diag.insert(
            "eligibility_rejected".into(),
            Value::from(diag_eligibility_rejected),
        );
        agent_diag.insert("eligible_count".into(), Value::from(diag_eligible));
        agent_diag.insert("claimed_this_loop".into(), Value::from(started));
        agent_diag.insert(
            "last_claim_attempt_at".into(),
            Value::from(isoformat_utc(Utc::now())),
        );

        if started > 0 {
            continue;
        }

        if idle_shutdown
            && slots.is_empty()
            && helpers::no_eligible_in_queue(
                &store,
                &sizing,
                &gpu_type,
                total_vram_gb,
                free_vram_gb,
                kind,
                &consumer_id,
                slots.len(),
            )
            .await?
        {
            log_fn("idle_shutdown: no slots + no eligible queued jobs; exiting");
            self_terminate(kind, log_fn).await;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot_info(job_id: &str, priority: i64, vram_gb: i64, age_s: u64) -> YieldSlotInfo {
        YieldSlotInfo {
            job_id: job_id.to_string(),
            priority,
            yieldable: true,
            exclusive: false,
            yield_count: 0,
            max_yields_before_protected: 5,
            started_mono: Instant::now()
                .checked_sub(Duration::from_secs(age_s))
                .unwrap_or_else(Instant::now),
            vram_gb,
        }
    }

    #[test]
    fn vram_safety_buffer_derives_from_total_with_floor() {
        assert_eq!(vram_safety_buffer_gb(96), 5); // ceil(96 * 0.05) = 5
        assert_eq!(vram_safety_buffer_gb(24), 4); // ceil(1.2) = 2 -> floor 4
        assert_eq!(vram_safety_buffer_gb(0), 4);
    }

    #[test]
    fn cuda_probe_result_prefers_stdout_and_truncates() {
        assert_eq!(
            cuda_probe_result(0, "cuda_available=True\n", ""),
            (true, "cuda_available=True".to_string())
        );
        assert_eq!(
            cuda_probe_result(66, "", "some torch error\n"),
            (false, "some torch error".to_string())
        );
        assert_eq!(cuda_probe_result(66, "", ""), (false, "rc=66".to_string()));
        // detail[-300:]: only the last 300 chars are kept.
        let long = "x".repeat(400);
        let (_, detail) = cuda_probe_result(1, &long, "");
        assert_eq!(detail.len(), 300);
    }

    #[test]
    fn choose_yield_evicts_lowest_priority_then_largest() {
        let now = Instant::now();
        let slots = vec![
            slot_info("a", 1, 10, 400),
            slot_info("b", 2, 12, 400),
            slot_info("c", 9, 40, 400), // priority >= target: never evicted
        ];
        // need=20, free=4: evict a (freed 10, still short), then b (22 >= 16).
        let chosen = choose_yield_slots(&slots, 5, 20, 4, now);
        assert_eq!(chosen, vec![0, 1]);
        // Only enough for the first slot: yield just one.
        let chosen = choose_yield_slots(&slots, 5, 12, 4, now);
        assert_eq!(chosen, vec![0]);
        // Even yielding everything evictable (22G) can't fit need=40 -> no yield.
        assert!(choose_yield_slots(&slots, 5, 40, 4, now).is_empty());
        // Equal priority: the larger slot goes first, often saving a yield.
        let equal = vec![slot_info("small", 1, 8, 400), slot_info("big", 1, 30, 400)];
        assert_eq!(choose_yield_slots(&equal, 5, 30, 4, now), vec![1]);
    }

    #[test]
    fn choose_yield_respects_protection_guards() {
        let now = Instant::now();
        // Too young (MIN_RUNTIME_BEFORE_YIELD_S anti-thrash floor).
        let young = vec![slot_info("young", 1, 50, 10)];
        assert!(choose_yield_slots(&young, 5, 20, 4, now).is_empty());
        // Yield-protected after max_yields_before_protected yields.
        let mut protected = slot_info("prot", 1, 50, 400);
        protected.yield_count = 5;
        assert!(choose_yield_slots(&[protected.clone()], 5, 20, 4, now).is_empty());
        // A stored 0 falls back to 5 (Python `getattr(...) or 5`).
        protected.yield_count = 4;
        protected.max_yields_before_protected = 0;
        assert_eq!(choose_yield_slots(&[protected], 5, 20, 4, now), vec![0]);
        // Exclusive slots are never evicted.
        let mut exclusive = slot_info("excl", 1, 50, 400);
        exclusive.exclusive = true;
        assert!(choose_yield_slots(&[exclusive], 5, 20, 4, now).is_empty());
        // Non-yieldable slots are never evicted.
        let mut pinned = slot_info("pinned", 1, 50, 400);
        pinned.yieldable = false;
        assert!(choose_yield_slots(&[pinned], 5, 20, 4, now).is_empty());
    }
}
