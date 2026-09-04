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
use std::path::{Path, PathBuf};
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

/// Claimable jobs one poll asks the queue for. It is a window over work this
/// agent may actually admit, not over the queue: the per-tick claim count is
/// bounded by slots, VRAM and `WC_LOCAL_MAX_CLAIMS_PER_TICK` further down.
const CLAIM_CANDIDATE_WINDOW: usize = 2_000;

/// Candidates the cooperative-yield scan considers before deciding what to
/// evict for.
const YIELD_CANDIDATE_WINDOW: usize = 200;

/// Job documents one scan may read while filling its window. Separating this
/// from the window is the whole point: a queue full of another host's work
/// costs scanning, and must not cost this host its candidates.
const QUEUE_SCAN_BUDGET: usize = 8_000;

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

const GPU_POWER_RECONCILE_INTERVAL_S: u64 = 300;

struct GpuPowerLimitState {
    desired_watts: u32,
    checked_at: Instant,
    checked_at_utc: String,
    ok: bool,
    detail: String,
}

/// How long between two reads of the host's placement policy file.
///
/// The worker re-reads the file itself every 30 seconds
/// (`CACHE_TTL_MS`, `placement-policy.ts`), so a reconcile slower than that
/// only delays when a registry edit takes effect, never how long a wrong file
/// stays in force once corrected. The same 300 the power limit uses: one
/// number for "how often this agent re-asserts a declaration".
const PLACEMENT_RECONCILE_INTERVAL_S: u64 = GPU_POWER_RECONCILE_INTERVAL_S;

/// The last placement-policy reconcile, so the pass is skipped while nothing
/// has changed and the outcome still reaches the capacity diagnostics.
struct PlacementPolicyState {
    /// `(enabled, actions)` last written or confirmed — what the worker acts
    /// on, which is the only part a rewrite would change.
    desired: (bool, Vec<String>),
    checked_at: Instant,
    checked_at_utc: String,
    ok: bool,
    detail: String,
}

async fn read_gpu_power_limits() -> Result<Vec<f64>, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=power.limit", "--format=csv,noheader,nounits"])
            .output(),
    )
    .await
    .map_err(|_| "nvidia-smi power query timed out after 30s".to_string())?
    .map_err(|error| format!("nvidia-smi power query failed: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "nvidia-smi power query exited {}: {}",
            output.status.code().unwrap_or(-1),
            detail.trim()
        ));
    }
    let limits = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.parse::<f64>()
                .map_err(|error| format!("invalid nvidia-smi power limit {line:?}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if limits.is_empty() {
        return Err("nvidia-smi power query returned no GPUs".to_string());
    }
    Ok(limits)
}

/// Put this host's registry-declared Weles policy into the file its worker
/// reads, and report `(detail, (enabled, actions))`.
///
/// The document is built by [`crate::cli::placement::policy_document`] — the
/// same builder `stado host publish-placement-policy` uses, so the bytes an
/// operator delivers from the coordinator and the bytes this writes are one
/// shape decided in one place.
///
/// Three properties the operator path also holds:
///
///   stamped     the generation is read with the document, and a read that
///               cannot produce one writes nothing. An unstamped policy is the
///               untraceable file this whole path exists to retire.
///   atomic      written to a temporary file in the destination directory and
///               renamed, so a worker reading concurrently sees the whole old
///               document or the whole new one.
///   quiet       a file whose entry already carries the declared `enabled` and
///               actions is left alone. `_source.published_at` moves on every
///               build, so comparing whole documents would rewrite the file
///               every pass and hand the worker a fresh mtime for no change.
async fn reconcile_placement_policy(
    target: &ComputeTarget,
) -> Result<(String, (bool, Vec<String>)), String> {
    let (_, generation) = crate::cli::registry::fetch_versioned_document()
        .await
        .map_err(|error| format!("registry generation unavailable: {error}"))?;
    let policy = crate::cli::placement::policy_document(
        target,
        &generation,
        crate::cli::placement::RECONCILED_BY,
    )
    .map_err(|error| error.to_string())?;
    let desired = crate::cli::placement::policy_effect(&policy);

    // The check `apply_policy` performs remotely, performed here for the same
    // reason: a policy whose entries name no identity of this machine does not
    // fail loudly in the worker. Its loader resolves to `enabled: false` and it
    // declines every row in silence — 29,616 times, the last time this fleet
    // learned it — so a writer that cannot see itself in the document must
    // write nothing and say why.
    let identity =
        crate::cli::placement::normalize_hostname(&crate::providers::vast::system_hostname());
    if !names_this_host(&policy, &identity) {
        return Err(format!(
            "the registry declares no identity matching {identity:?} for target {}, so the \
             policy built from it would name every host except this one and the worker would \
             decline every routed row. Declare {identity:?} in that target's `hostnames`",
            target.name
        ));
    }

    let path = placement_policy_path()?;
    let held = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    if let Some(held) = &held {
        if crate::cli::placement::policy_effect(held) == desired {
            return Ok((
                format!("{} already carries the declaration", path.display()),
                desired,
            ));
        }
    }

    let directory = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let staged = directory.join(format!(".{}.stado-agent", PLACEMENT_POLICY_FILE));
    let bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(&policy).map_err(|error| error.to_string())?
    );
    std::fs::write(&staged, bytes)
        .map_err(|error| format!("cannot stage {}: {error}", staged.display()))?;
    std::fs::rename(&staged, &path).map_err(|error| {
        let _ = std::fs::remove_file(&staged);
        format!("cannot install {}: {error}", path.display())
    })?;
    Ok((
        format!(
            "wrote {} at registry generation {generation}",
            path.display()
        ),
        desired,
    ))
}

/// Whether `policy` carries an entry this machine's worker will match, by the
/// loader's own rule: `hostname` equal, or `identity` present in `aliases`,
/// both normalized (`placement-policy.ts`).
fn names_this_host(policy: &Value, identity: &str) -> bool {
    policy
        .get("hosts")
        .and_then(Value::as_array)
        .is_some_and(|hosts| {
            hosts.iter().any(|host| {
                let named = host
                    .get("hostname")
                    .and_then(Value::as_str)
                    .is_some_and(|name| {
                        crate::cli::placement::normalize_hostname(name) == identity
                    });
                named
                    || host
                        .get("aliases")
                        .and_then(Value::as_array)
                        .is_some_and(|aliases| {
                            aliases.iter().filter_map(Value::as_str).any(|alias| {
                                crate::cli::placement::normalize_hostname(alias) == identity
                            })
                        })
            })
        })
}

/// Basename of the worker's policy file, per `placement-policy.ts`.
const PLACEMENT_POLICY_FILE: &str = "placement-policy.json";

/// Where the worker reads it: `WELES_PLACEMENT_POLICY_FILE` when set, else
/// `$HOME/.config/weles/placement-policy.json`. The override is honoured
/// because a writer that ignores it writes a file nobody reads.
fn placement_policy_path() -> Result<PathBuf, String> {
    if let Some(override_path) = std::env::var_os("WELES_PLACEMENT_POLICY_FILE") {
        let path = PathBuf::from(override_path);
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "HOME is unset, so the worker's policy path is unknown".to_string())?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("weles")
        .join(PLACEMENT_POLICY_FILE))
}

/// Reconcile the host-level NVIDIA board power cap declared in the registry.
/// Existing jobs keep running on failure, but the caller publishes zero free
/// capacity until the driver accepts and reports the declared limit.
pub async fn reconcile_gpu_power_limit(watts: u32) -> Result<String, String> {
    let desired = f64::from(watts);
    let current = read_gpu_power_limits().await?;
    if !current.iter().all(|actual| (actual - desired).abs() < 0.5) {
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new("nvidia-smi")
                .arg(format!("--power-limit={watts}"))
                .output(),
        )
        .await
        .map_err(|_| "nvidia-smi power-limit update timed out after 30s".to_string())?
        .map_err(|error| format!("nvidia-smi power-limit update failed: {error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "nvidia-smi power-limit update exited {}: {}",
                output.status.code().unwrap_or(-1),
                detail.trim()
            ));
        }
    }
    let actual = read_gpu_power_limits().await?;
    if !actual.iter().all(|value| (*value - desired).abs() < 0.5) {
        return Err(format!(
            "driver reported power limits {actual:?} W after requesting {watts} W"
        ));
    }
    Ok(format!("{actual:?} W"))
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
    // The window counts jobs THIS agent could admit. Counting merely fitting
    // jobs let a page of another worker's or another platform's work fill it
    // and hid the higher-priority job this host is meant to make room for.
    let mut candidates = store
        .list_claimable_jobs(
            "queue",
            &crate::queue::listing::JobScan {
                want: YIELD_CANDIDATE_WINDOW,
                scan_budget: QUEUE_SCAN_BUDGET,
                max_gpu_mem_gb: total_vram_gb,
                eligible: &|job| {
                    helpers::job_eligible(
                        job,
                        gpu_type,
                        total_vram_gb,
                        kind,
                        consumer_id,
                        slots.len(),
                        false,
                    )
                },
                // Preempting a running job is a priority-fidelity decision,
                // not a reachability one: it must be taken against the head
                // of the index. The rotating cursor is shared with the claim
                // loops below, so inheriting it would answer "the most
                // important job in some rotated slice", and this host would
                // yield to the wrong job — or fail to yield at all while the
                // job it should make room for sat before the slice.
                from_head: true,
            },
        )
        .await?;
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

// An admission decision needs the whole picture at once: store, sizing, device,
// capacity and kind are each read on a different branch below.
#[allow(clippy::too_many_arguments)]
async fn queued_gpu_job_for_inference(
    store: &JobStorage,
    sizing: &Sizing,
    gpu_type: &str,
    total_vram_gb: i64,
    kind: &str,
    consumer_id: &str,
    active_slot_count: usize,
    pinned_only: bool,
) -> Result<Option<(String, i64)>, StorageError> {
    let listed = store
        .list_claimable_jobs(
            "queue",
            &crate::queue::listing::JobScan {
                want: CLAIM_CANDIDATE_WINDOW,
                scan_budget: QUEUE_SCAN_BUDGET,
                max_gpu_mem_gb: total_vram_gb,
                eligible: &|job| {
                    helpers::job_eligible(
                        job,
                        gpu_type,
                        total_vram_gb,
                        kind,
                        consumer_id,
                        active_slot_count,
                        pinned_only,
                    )
                },
                // A claim loop wants reachability and so takes the shared
                // rotation: a job past this poll's window is reached by a
                // later poll rather than never.
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
    queued.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.created_at.cmp(&right.created_at))
    });
    for job in queued {
        let need = job
            .gpu_mem_gb
            .max(estimate_gpu_memory(&job.command, sizing, store).await?);
        if need <= 0
            || !helpers::job_eligible(
                &job,
                gpu_type,
                total_vram_gb,
                kind,
                consumer_id,
                active_slot_count,
                pinned_only,
            )
        {
            continue;
        }
        return Ok(Some((job.job_id, need)));
    }
    Ok(None)
}

fn inference_container_name(deployment: &str) -> Result<String, String> {
    let valid = !deployment.is_empty()
        && deployment.len() <= 128
        && deployment.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_".contains(&byte)
        });
    valid
        .then(|| format!("stado-inference-{deployment}"))
        .ok_or_else(|| "inference reservation contains an invalid deployment name".to_string())
}

async fn inference_container_running(deployment: &str) -> Result<bool, String> {
    let container = inference_container_name(deployment)?;
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new("docker")
            .args(["inspect", "--format={{.State.Running}}", &container])
            .output(),
    )
    .await
    .map_err(|_| "docker inspect timed out".to_string())?
    .map_err(|error| format!("docker inspect failed: {error}"))?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}

async fn set_inference_container_running(deployment: &str, running: bool) -> Result<(), String> {
    let container = inference_container_name(deployment)?;
    let mut command = tokio::process::Command::new("docker");
    if running {
        command.args(["start", &container]);
    } else {
        command.args(["stop", "--time", "30", &container]);
    }
    let output = tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .map_err(|_| "docker inference transition timed out".to_string())?
        .map_err(|error| format!("docker inference transition failed: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "docker inference transition exited {}: {}",
        output.status,
        detail.trim().chars().take(400).collect::<String>()
    ))
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

/// Publish one capacity broadcast and say, in the log, which branch of the loop
/// produced it and whether the store accepted it.
///
/// The loop below can leave its iteration nine ways and used to name none of
/// them. On the always-on mac that cost seven days: the log showed
/// `loop: iter-start` and a disk-cleanup report every ten seconds, nothing
/// after, and the broadcast in the fleet store stayed frozen at a timestamp
/// three minutes before the agent's unit was re-declared. Both facts were
/// consistent with about four different branches and with a crash-restart loop,
/// and separating them took a census of a log that should simply have said.
///
/// Two of the publish sites also discarded the store's answer with `let _ =`, so
/// an agent whose every write was being refused reported exactly what an agent
/// with nothing to say reports. The result is returned to the caller, and the
/// failure is logged here either way, because a broadcast nobody accepted is the
/// one event this process exists to perform.
#[allow(clippy::too_many_arguments)]
async fn publish_branch(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    branch: &str,
    free_slots: &BTreeMap<String, i64>,
    free_vram_gb: Option<i64>,
    total_vram_gb: Option<i64>,
    diag: Option<Map<String, Value>>,
    log_fn: &mut dyn FnMut(&str),
) -> Result<(), StorageError> {
    let outcome = publish_capacity(
        store,
        consumer_id,
        kind,
        free_slots,
        free_vram_gb,
        total_vram_gb,
        diag,
    )
    .await;
    match &outcome {
        Ok(()) => log_fn(&format!(
            "loop: {branch}: published free_vram_gb={} free_slots={}",
            free_vram_gb.unwrap_or_default(),
            free_slots.values().sum::<i64>()
        )),
        Err(exc) => log_fn(&format!(
            "loop: {branch}: capacity publish REFUSED by the store: {exc}"
        )),
    }
    outcome
}

const INSTALLED_STADO_RELEASE_VERSION: &str = "stado.release-version";

/// Tells the command wrapper to end the process so its declared supervisor can
/// start the installed Stado image. Ordinary loop errors remain retryable.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct ReleaseHandoff(String);

/// What the managed binary on disk says it is, asked of the file itself.
fn managed_binary_version(managed: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new(managed)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.split_whitespace().last().map(str::to_string)
}

/// A release handoff worth taking: the marker names a release this process is
/// not, AND the binary on disk is no longer this process's image -- so exiting
/// hands control to a genuinely different file and the supervisor's restart
/// changes something.
///
/// The marker alone cannot decide that, and reading it as authoritative cost a
/// host its entire share of the queue. On 2026-09-02 `lukasz-macbook` held a
/// 0.13.42 binary at the managed path beside a marker still reading 0.13.39.
/// The agent announced `installed 0.13.39 supersedes running 0.13.42`, exited,
/// was recreated by launchd from that same file, and repeated it every ten
/// seconds -- claiming nothing, while a signed Stado release delivery pinned to
/// that host sat queued behind it. A handoff whose restart cannot change the
/// running image is not a handoff, it is a stall with an explanation.
///
/// So the file is asked what it is. When it answers this process's own version
/// the marker is merely stale: it is corrected here, once, instead of being
/// re-read forever. Only a file that answers something else is a release
/// waiting to be started. Source-tree recovery agents stay excluded, because
/// only an agent launched through the owner-managed binary belongs to a
/// supervisor that recreates it.
fn installed_stado_release_mismatch(log_fn: &mut impl FnMut(&str)) -> Option<String> {
    let home = crate::config_file::expand_tilde("~");
    let managed = home.join(".stado").join("bin").join("stado");
    let argv0 = std::env::args_os().next().map(std::path::PathBuf::from)?;
    if argv0 != managed {
        return None;
    }
    let marker = home
        .join(".stado")
        .join("bin")
        .join(INSTALLED_STADO_RELEASE_VERSION);
    let installed = std::fs::read_to_string(&marker).ok()?;
    let installed = installed.trim();
    let running = env!("CARGO_PKG_VERSION");
    if installed.is_empty() || installed == running {
        return None;
    }
    match managed_binary_version(&managed) {
        Some(on_disk) if on_disk != running => Some(installed.to_string()),
        Some(on_disk) => {
            match std::fs::write(&marker, format!("{on_disk}\n")) {
                Ok(()) => log_fn(&format!(
                    "loop: release-marker-repaired: {} named {installed} while the managed binary \
                     is {on_disk}; the marker now names what is installed",
                    marker.display()
                )),
                Err(error) => log_fn(&format!(
                    "loop: release-marker-stale: {} names {installed} while the managed binary is \
                     {on_disk}, and it could not be corrected: {error}",
                    marker.display()
                )),
            }
            None
        }
        None => {
            log_fn(&format!(
                "loop: release-marker-unverified: {} names {installed} and the managed binary \
                 would not state its version; continuing rather than handing off to an image that \
                 may not differ",
                marker.display()
            ));
            None
        }
    }
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
    // Which store this agent just bound to, and whether a coordinate written
    // there means anything to anyone else.
    //
    // A capacity broadcast is a claim about the fleet. Written into a store
    // whose reach is `Device` the write does not fail -- it succeeds, and every
    // other host reports the broadcast absent, which is indistinguishable from
    // an agent that never ran. That is what the always-on mac did for an
    // afternoon: its unit was re-declared carrying `STADO_CONFIG`, the host
    // configuration behind that path selects `storage.backend: local`, and from
    // that moment the agent published into a directory on its own disk while the
    // scheduler read a frozen row and 55 jobs pinned to that host waited on a
    // capacity number nobody was writing any more.
    //
    // The agent keeps running: it is also the host's disk janitor, and stopping
    // cleanup on a machine under disk pressure trades one outage for another.
    // What it must not do is stay quiet about it. The store is named on every
    // iteration, in the log, which is the one channel that still reaches an
    // operator when the store itself is the thing that is wrong.
    let storage_backend = crate::config::wc_storage_backend();
    let store_reach = crate::capabilities::storage_reach(storage_backend);
    let store_answers_for_fleet = store_reach == Some(crate::capabilities::StorageReach::Fleet);
    log_fn(&format!(
        "init: capacity broadcasts go to the {storage_backend:?} store, which answers for {}",
        match store_reach {
            Some(crate::capabilities::StorageReach::Fleet) => "the fleet",
            Some(crate::capabilities::StorageReach::Device) => "this machine only",
            None => "an unknown scope: this build does not know that backend",
        }
    ));
    let sizing = Sizing::new();
    let consumer_id = format!("{kind}-{hostname}");
    let mut slots: Vec<ActiveSlot> = Vec::new();
    let mut agent_diag: Map<String, Value> = Map::new();
    let fleet_staging = std::env::var("STADO_HF_FLUSH_STAGING_DIR")
        .ok()
        .filter(|path| !path.trim().is_empty());
    let mut last_fleet_flush = Instant::now();

    let mut last_cap: Option<LastCap> = None;
    let mut gpu_power_limit_state: Option<GpuPowerLimitState> = None;
    let mut placement_policy_state: Option<PlacementPolicyState> = None;
    let mut pinned_only = false; // registry ComputeTarget.pinned_only, refreshed per poll
                                 // Python `disk_low_bytes = _persisted_disk_low_bytes()`: reuse the last
                                 // canonical low watermark from the janitor's owner-controlled state
                                 // file (cleanup may be unable to reach the registry during startup).
    let mut disk_low_bytes = disk_cleanup::persisted_disk_low_bytes();
    if disk_low_bytes.is_some() {
        log_fn("init: loaded validated disk low watermark from janitor state");
    }
    // The janitor owns its own cadence from here. It is still invoked at the
    // tick's poll interval -- the cleanup engine's own lock and policy decide
    // what a pass does -- but off the critical path, so a long pass delays only
    // the next pass and never a capacity broadcast. Held in scope for the
    // agent's lifetime: dropping the handle aborts the pass loop, so a release
    // handoff does not leave a janitor behind.
    let janitor_reports = crate::providers::local::agent_janitor::JanitorReports::new();
    let _janitor = janitor_reports.spawn_janitor(
        std::time::Duration::from_secs(crate::constants::POLL_INTERVAL_S),
        |active_slots| async move {
            disk_cleanup::run_cleanup_once(
                active_slots,
                false,
                disk_cleanup::CleanupWriter::AgentTick,
                &mut |msg: &str| agent_log(msg),
            )
            .await
        },
    );
    loop {
        // Phase breadcrumbs for the 40GB a2-highgpu-1g first-iter hang.
        log_fn("loop: iter-start");
        // Every broadcast says which store wrote it. A reader holding a frozen
        // row could not tell a stopped agent from a running one publishing
        // somewhere else, and that is the question that took an afternoon.
        agent_diag.insert("storage_backend".into(), Value::from(storage_backend));
        agent_diag.insert(
            "storage_answers_for_fleet".into(),
            Value::from(store_answers_for_fleet),
        );
        if !store_answers_for_fleet {
            log_fn(&format!(
                "loop: this agent's {storage_backend:?} store does not answer for the fleet, so \
                 every capacity broadcast below is invisible to the scheduler and to every other \
                 host; the queue it reads is not the fleet queue"
            ));
        }
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
        if slots.is_empty() {
            if let Some(installed) = installed_stado_release_mismatch(log_fn) {
                let detail = format!(
                    "installed Stado {installed} supersedes running {}; exiting after all slots \
                     finished so the declared supervisor starts the installed release",
                    env!("CARGO_PKG_VERSION")
                );
                log_fn(&format!("loop: release-handoff: {detail}"));
                // Linux services use Restart=on-failure; launchd KeepAlive also
                // recreates this process. The command wrapper must propagate
                // this typed error instead of treating it as a retryable loop
                // failure inside the same process.
                return Err(ReleaseHandoff(detail).into());
            }
        }
        // The janitor's bounded cleanup pass runs on its own task
        // ([`super::agent_janitor`]); this tick only reads passes that have
        // already finished. Awaiting the pass here is what made a release
        // builder invisible: a 13.6-minute `healthy_noop` pass held the
        // capacity publication far past the 180s staleness cutoff, so
        // `release submit` refused a healthy, correctly declared builder.
        // Publication must happen at the heartbeat interval whatever the
        // janitor is doing, so nothing on this line may ever wait for it.
        janitor_reports.set_active_slots(slots.len() as i64);
        if let Some(cleanup_report) = janitor_reports.latest() {
            if let Some(reported_low) = disk_cleanup::validated_report_low_bytes(&cleanup_report) {
                disk_low_bytes = Some(reported_low);
            }
            agent_diag.insert("disk_cleanup".into(), cleanup_report);
        } else {
            // No pass has completed yet. Say so rather than leaving the key
            // absent, which reads identically to a wedged janitor.
            agent_diag.insert(
                "disk_cleanup".into(),
                serde_json::json!({"outcome": "no_pass_completed_yet"}),
            );
        }
        // Admission reads the canonical declaration directly as well as the
        // janitor report. Cleanup deliberately uses a cross-process lock; a
        // busy lock or an older writer's invalid report must not erase a
        // perfectly readable low watermark and close the queue forever.
        let registry_target = lookup_self_auto(&hostname).await?;
        if let Some(declared_low) = registry_target
            .as_ref()
            .and_then(|target| target.disk_cleanup.as_ref())
            .map(|policy| policy.low_free_gb.saturating_mul(disk_cleanup::GIB))
        {
            if disk_low_bytes != Some(declared_low) {
                log_fn("loop: loaded disk low watermark from the canonical registry");
            }
            disk_low_bytes = Some(declared_low);
        }
        // Python: shutil.disk_usage(expanduser("~")).free, OSError -> None.
        let current_free_bytes =
            disk_cleanup::free_bytes(&crate::config_file::expand_tilde("~")).ok();
        // Two different questions used to share one answer, and the conflation
        // is what froze the always-on mac. "Can this agent read its disk policy
        // at all" is a reason to fail admission closed: an agent that does not
        // know its own threshold cannot judge anything. "Is free space below the
        // janitor's low watermark" is not that. It is the janitor's cue to start
        // deleting, and on a host whose cleaners have nothing eligible to delete
        // -- every cleaner on that mac reported zero eligible items -- it is a
        // condition no cleanup pass can clear, so treating it as an admission
        // gate stopped the host permanently and silently: 19.6 GiB free against
        // a 20 GiB watermark, a zero-capacity publish, `continue`, forever.
        //
        // So pressure no longer suppresses the BROADCAST. It still suppresses
        // claiming, and the first version of this change did not, which was
        // wrong: within forty minutes of the same host being put back on the
        // fleet store its free space fell 19.3 -> 17.0 -> 13.8 GiB, because the
        // queue it had started draining is full of `cargo build` workloads and
        // the jobs themselves are what consume the disk. The gates that measure
        // actual consumption do not cover them -- the `$HOME` write probe only
        // fails once the disk is already full, and the raw-disk reserve applies
        // to activation-extraction jobs alone -- so removing the watermark from
        // admission would have let the host claim its way to zero.
        //
        // The defect was never that pressure stops claiming. It was that a host
        // which stops claiming says nothing at all: the broadcast went to zero
        // and the row went stale, so the fleet could not distinguish "under its
        // disk watermark" from "dead". Capacity is now published every loop with
        // `disk_pressure_active` in the diagnostics, and `host gates` reports the
        // numbers, so the operator gets a reason instead of a silence.
        let disk_policy_known = disk_low_bytes.is_some();
        let readings_incomplete = disk_low_bytes.is_none() || current_free_bytes.is_none();
        let pressure_active =
            disk_cleanup::disk_pressure_active(disk_low_bytes, current_free_bytes);
        agent_diag.insert(
            "disk_cleanup_policy_known".into(),
            Value::from(disk_policy_known),
        );
        // The key keeps its published name: `host gates` reads it to say the
        // agent is refusing to claim because it cannot read its disk policy, and
        // that is now exactly what it means and nothing more.
        agent_diag.insert(
            "disk_pressure_unresolved".into(),
            Value::from(readings_incomplete),
        );
        agent_diag.insert("disk_pressure_active".into(), Value::from(pressure_active));
        if readings_incomplete {
            let zero_diag = agent_diag.clone();
            log_fn(&format!(
                "loop: disk-policy-unreadable: low watermark {} and free space {} -- failing \
                 admission closed until both are known",
                disk_low_bytes.map_or("unknown".to_string(), |bytes| bytes.to_string()),
                current_free_bytes.map_or("unknown".to_string(), |bytes| bytes.to_string())
            ));
            let _ = publish_branch(
                &store,
                &consumer_id,
                kind,
                "disk-policy-unreadable",
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(zero_diag.clone()),
                log_fn,
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
            let _ = publish_branch(
                &store,
                &consumer_id,
                kind,
                "keep-alive-republish",
                &cap.free_slots,
                Some(cap.free_vram_gb),
                Some(cap.total_vram_gb),
                Some(cap.diag.clone()),
                log_fn,
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
        let mut gpu_power_policy_ok = true;
        if let Some(t) = registry_target.as_ref() {
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
                if let Some(watts) = t.gpu_power_limit_watts() {
                    let reconcile_due = gpu_power_limit_state.as_ref().is_none_or(|state| {
                        state.desired_watts != watts
                            || !state.ok
                            || state.checked_at.elapsed()
                                >= Duration::from_secs(GPU_POWER_RECONCILE_INTERVAL_S)
                    });
                    if reconcile_due {
                        let checked_at_utc = isoformat_utc(Utc::now());
                        let result = reconcile_gpu_power_limit(watts).await;
                        let (ok, detail) = match result {
                            Ok(detail) => (true, detail),
                            Err(detail) => {
                                log_fn(&format!("GPU power-limit reconciliation failed: {detail}"));
                                (false, detail)
                            }
                        };
                        gpu_power_limit_state = Some(GpuPowerLimitState {
                            desired_watts: watts,
                            checked_at: Instant::now(),
                            checked_at_utc,
                            ok,
                            detail,
                        });
                    }
                    if let Some(state) = &gpu_power_limit_state {
                        gpu_power_policy_ok = state.ok;
                        agent_diag.insert(
                            "gpu_power_limit_watts".into(),
                            Value::from(state.desired_watts),
                        );
                        agent_diag.insert("gpu_power_limit_ok".into(), Value::from(state.ok));
                        agent_diag.insert(
                            "gpu_power_limit_checked_at".into(),
                            Value::from(state.checked_at_utc.clone()),
                        );
                        agent_diag.insert(
                            "gpu_power_limit_detail".into(),
                            Value::from(state.detail.clone()),
                        );
                    }
                } else {
                    gpu_power_limit_state = None;
                    for key in [
                        "gpu_power_limit_watts",
                        "gpu_power_limit_ok",
                        "gpu_power_limit_checked_at",
                        "gpu_power_limit_detail",
                    ] {
                        agent_diag.remove(key);
                    }
                }
                // The registry declares `weles.actions` per host and the
                // worker reads its own `placement-policy.json`. Until this
                // ran, the only thing joining them was an operator typing
                // `stado host publish-placement-policy`, so the two drifted
                // silently: the registry listed an action the file did not
                // and the worker declined every routed row while the
                // declaration said it was allowed. Asserted here for the same
                // reason the power limit is — a declaration nothing enforces
                // is a declaration written for nobody.
                if t.weles.is_some() {
                    let reconcile_due = placement_policy_state.as_ref().is_none_or(|state| {
                        !state.ok
                            || state.checked_at.elapsed()
                                >= Duration::from_secs(PLACEMENT_RECONCILE_INTERVAL_S)
                    });
                    if reconcile_due {
                        let checked_at_utc = isoformat_utc(Utc::now());
                        let (ok, detail, desired) = match reconcile_placement_policy(t).await {
                            Ok((detail, desired)) => (true, detail, desired),
                            Err(detail) => {
                                log_fn(&format!(
                                    "placement-policy reconciliation failed: {detail}"
                                ));
                                (false, detail, (false, Vec::new()))
                            }
                        };
                        placement_policy_state = Some(PlacementPolicyState {
                            desired,
                            checked_at: Instant::now(),
                            checked_at_utc,
                            ok,
                            detail,
                        });
                    }
                    if let Some(state) = &placement_policy_state {
                        agent_diag.insert(
                            "placement_policy_enabled".into(),
                            Value::from(state.desired.0),
                        );
                        agent_diag.insert(
                            "placement_policy_actions".into(),
                            Value::from(state.desired.1.clone()),
                        );
                        agent_diag.insert("placement_policy_ok".into(), Value::from(state.ok));
                        agent_diag.insert(
                            "placement_policy_checked_at".into(),
                            Value::from(state.checked_at_utc.clone()),
                        );
                        agent_diag.insert(
                            "placement_policy_detail".into(),
                            Value::from(state.detail.clone()),
                        );
                    }
                } else {
                    // A host that declares no `weles` block has no policy to
                    // assert, and the file it may already carry is not this
                    // agent's to remove: deleting it would take a worker out
                    // on the strength of an absent declaration.
                    placement_policy_state = None;
                    for key in [
                        "placement_policy_enabled",
                        "placement_policy_actions",
                        "placement_policy_ok",
                        "placement_policy_checked_at",
                        "placement_policy_detail",
                    ] {
                        agent_diag.remove(key);
                    }
                }
            }
        }
        if !gpu_power_policy_ok {
            publish_branch(
                &store,
                &consumer_id,
                kind,
                "gpu-power-policy-unmet",
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(agent_diag.clone()),
                log_fn,
            )
            .await?;
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
            continue;
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
            publish_branch(
                &store,
                &consumer_id,
                kind,
                "vast-renter-active",
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(agent_diag.clone()),
                log_fn,
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
        // Every card, one driver read. `free_vram_gb` stays the answer to "will
        // one job fit", which is the emptiest board; the per-card list carries
        // the rest of the truth into the broadcast and into device selection,
        // because a host with two boards has two independent pools and the
        // pooled reading was wrong about both.
        let mut cards = helpers::smi_gpu_cards().await;
        cards.sort_by_key(|card| std::cmp::Reverse(card.free_vram_gb));
        let smi_free = cards
            .iter()
            .map(|card| card.free_vram_gb)
            .max()
            .unwrap_or(-1);
        if smi_free >= 0 && smi_free < free_vram_gb {
            free_vram_gb = smi_free;
        }
        if let Some(reservation) = &inference_reservation {
            agent_diag.insert(
                "inference_reservation".into(),
                Value::from(reservation.deployment.clone()),
            );
            agent_diag.insert(
                "inference_gpu_mode".into(),
                Value::from(reservation.gpu_mode.clone()),
            );
            if reservation.gpu_mode == crate::inference::schema::GPU_EXCLUSIVE {
                free_vram_gb = 0;
                log_fn(&format!(
                    "exclusive inference reservation '{}': GPU claims disabled; CPU-only claims remain eligible",
                    reservation.deployment
                ));
            } else {
                let queued_job = queued_gpu_job_for_inference(
                    &store,
                    &sizing,
                    &gpu_type,
                    total_vram_gb,
                    kind,
                    &consumer_id,
                    slots.len(),
                    pinned_only,
                )
                .await?;
                let gpu_work_active = used_vram > 0;
                let should_yield = gpu_work_active || queued_job.is_some();
                match inference_container_running(&reservation.deployment).await {
                    Ok(true) if should_yield => {
                        let reason = queued_job
                            .as_ref()
                            .map(|(job_id, need)| format!("queued job {job_id} needs {need} GiB"))
                            .unwrap_or_else(|| "an admitted GPU job is active".to_string());
                        log_fn(&format!(
                            "yieldable inference '{}': pausing because {reason}",
                            reservation.deployment
                        ));
                        match set_inference_container_running(&reservation.deployment, false).await
                        {
                            Ok(()) => continue,
                            Err(error) => {
                                log_fn(&format!(
                                    "yieldable inference '{}': pause failed safely: {error}",
                                    reservation.deployment
                                ));
                                free_vram_gb = 0;
                            }
                        }
                    }
                    Ok(true) => {
                        free_vram_gb = 0;
                        agent_diag.insert("inference_runtime_state".into(), Value::from("serving"));
                    }
                    Ok(false) if !should_yield && slots.is_empty() => {
                        agent_diag
                            .insert("inference_runtime_state".into(), Value::from("resuming"));
                        publish_branch(
                            &store,
                            &consumer_id,
                            kind,
                            "inference-resuming",
                            &BTreeMap::new(),
                            Some(0),
                            Some(total_vram_gb),
                            Some(agent_diag.clone()),
                            log_fn,
                        )
                        .await?;
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
                        continue;
                    }
                    Ok(false) => {
                        agent_diag.insert("inference_runtime_state".into(), Value::from("yielded"));
                        if let Some((job_id, need)) = queued_job {
                            agent_diag
                                .insert("inference_yield_for_job".into(), Value::from(job_id));
                            agent_diag
                                .insert("inference_yield_for_vram_gb".into(), Value::from(need));
                        }
                    }
                    Err(error) => {
                        free_vram_gb = 0;
                        agent_diag.insert("inference_runtime_state".into(), Value::from("unknown"));
                        log_fn(&format!(
                            "yieldable inference '{}': runtime state unavailable; GPU claims disabled: {error}",
                            reservation.deployment
                        ));
                    }
                }
            }
        }
        let (refuse_disk, disk_diag) = disk_gate::gate_and_maybe_evict(log_fn);
        agent_diag.extend(diag_map(&disk_diag));
        if refuse_disk {
            publish_branch(
                &store,
                &consumer_id,
                kind,
                "disk-gate-refused",
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(agent_diag.clone()),
                log_fn,
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
            publish_branch(
                &store,
                &consumer_id,
                kind,
                "slots-settling-for-vram",
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(agent_diag.clone()),
                log_fn,
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
            publish_branch(
                &store,
                &consumer_id,
                kind,
                "vram-below-safety-buffer",
                &BTreeMap::new(),
                Some(0),
                Some(total_vram_gb),
                Some(agent_diag.clone()),
                log_fn,
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
                publish_branch(
                    &store,
                    &consumer_id,
                    kind,
                    "nvidia-driver-probe-failed",
                    &BTreeMap::new(),
                    Some(0),
                    Some(total_vram_gb),
                    Some(agent_diag.clone()),
                    log_fn,
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
        // A policy refusal above sets `free_vram_gb` to 0 and falls through to
        // this publish, so it decides whether any card is offered at all; the
        // per-card frees decide how many.
        let broadcast_cards: Vec<i64> = if free_vram_gb <= 0 {
            Vec::new()
        } else {
            cards.iter().map(|card| card.free_vram_gb).collect()
        };
        agent_diag.insert("gpu_cards".into(), Value::from(cards.len() as i64));
        agent_diag.insert(
            "gpu_free_vram_gb_per_card".into(),
            Value::from(
                cards
                    .iter()
                    .map(|card| Value::from(card.free_vram_gb))
                    .collect::<Vec<_>>(),
            ),
        );
        let free_slots = helpers::with_cpu_capacity(
            helpers::build_capacity_dict_per_card(&gpu_type, &broadcast_cards, total_vram_gb),
            hard_slot_cap,
            slots.len(),
        );
        publish_branch(
            &store,
            &consumer_id,
            kind,
            "claim-loop-open",
            &free_slots,
            Some(free_vram_gb),
            Some(total_vram_gb),
            Some(agent_diag.clone()),
            log_fn,
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
        // Which build is answering for this host. The broadcast carried a
        // capacity verdict, a claim-loop census and a disk report and never the
        // version that produced them, so after `host release` installed 0.9.5
        // and `service converge` reported `installed 0.9.5, in-sync`, there was
        // no way to tell whether the process still refusing every pinned job
        // was the new binary or one of the older ones the same host was running
        // — and the question had to be answered by reading process ages out of
        // `ps`, on a machine whose pid counter had wrapped.
        // The version alone did not finish the job. `0.14.6` named four
        // different trees of this crate on 2026-09-03, and a host publishing
        // `agent_version: "0.14.6"` still left "which build is this" to be
        // answered by reading symbols out of the binary. The identity carries
        // the revision, and the revision is published beside it so a reader
        // does not have to parse the sentence to get at it.
        agent_diag.insert(
            "agent_version".into(),
            Value::from(crate::build_identity::BUILD_IDENTITY),
        );
        agent_diag.insert(
            "agent_source_revision".into(),
            Value::from(crate::build_identity::SOURCE_REVISION),
        );
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
        // Disk pressure is enforced after the assigned queue is read. One exact
        // signed release-delivery command remains admissible so Stado can repair
        // the agent binary that owns this gate; every ordinary workload remains
        // blocked below the watermark.

        // Cooperative yield: if a higher-priority queued job can't fit, evict
        // just enough lower-priority yieldable slots to make room. Runs BEFORE
        // the full-GPU early-return below because that is exactly when it's
        // needed. Inert (single any() over slots) unless a yieldable job runs.
        if !pressure_active
            && maybe_yield_for_priority(
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
        // MemAvailable already excludes RAM resident in every non-agent
        // process. Adding those processes' RSS again double-reserves the same
        // memory and can permanently wedge a healthy host once its baseline
        // load exceeds half of physical RAM. Keep only the dynamic headroom;
        // per-job memory remains represented by Job::memory_gb.
        let fr = helpers::free_ram_gb();
        let ram_reserve = helpers::ram_safety_buffer_gb();
        if (0.0..ram_reserve).contains(&fr) {
            log_fn(&format!(
                "RAM gate: {} GB free < {} GB reserve; skipping claims",
                fr as i64, ram_reserve as i64
            ));
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }

        // Centralized assignment writes job.assigned_to on the queue blob, so
        // the listing itself applies this agent's full admission rule: the
        // window must count jobs this host may claim. Counting jobs that
        // merely fit its VRAM meant a fleet whose oldest two thousand fitting
        // jobs were assigned elsewhere handed this agent nothing claimable on
        // every poll, forever, while its own assigned job sat past the window.
        // The re-read below re-applies the rule to the FRESH document, which
        // is a different fact from the listed snapshot.
        let listed = store
            .list_claimable_jobs(
                "queue",
                &crate::queue::listing::JobScan {
                    want: CLAIM_CANDIDATE_WINDOW,
                    scan_budget: QUEUE_SCAN_BUDGET,
                    max_gpu_mem_gb: free_vram_gb,
                    eligible: &|job| {
                        helpers::job_eligible(
                            job,
                            &gpu_type,
                            total_vram_gb,
                            kind,
                            &consumer_id,
                            slots.len(),
                            pinned_only,
                        )
                    },
                    // A claim loop wants reachability and so takes the shared
                    // rotation: a job past this poll's window is reached by a
                    // later poll rather than never.
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
            queued.retain(|job| {
                job.gpu_mem_gb == 0
                    && job.priority == crate::constants::RELEASE_JOB_PRIORITY
                    && !job.run_id.is_empty()
                    && !job.pinned_host.is_empty()
                    && job.command == crate::constants::RELEASE_DELIVERY_JOB_COMMAND
                    && job
                        .output_uri
                        .starts_with("stado://probierz/runs/release-pipeline/stado/")
                    && job.output_uri.contains("/deliveries/")
                    && job.output_uri.ends_with("/output")
            });
            // A host can accumulate deliveries while it is under pressure.
            // The newest submission is the current operator intent; replaying
            // them FIFO briefly downgrades the installed agent before climbing
            // through every superseded coordinate.
            let matched_deliveries = queued.len();
            queued.sort_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| b.job_id.cmp(&a.job_id))
            });
            queued.truncate(1);
            agent_diag.insert(
                "disk_pressure_superseded_deliveries".into(),
                Value::from(matched_deliveries.saturating_sub(queued.len()) as i64),
            );
            agent_diag.insert(
                "disk_pressure_recovery_jobs".into(),
                Value::from(queued.len() as i64),
            );
            if queued.is_empty() {
                log_fn(&format!(
                    "loop: disk-pressure-active: {} bytes free is under the {} byte low \
                     watermark; ordinary work remains blocked and no signed Stado release \
                     delivery is assigned to this host",
                    current_free_bytes.unwrap_or_default(),
                    disk_low_bytes.unwrap_or_default()
                ));
                tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
                continue;
            }
            log_fn(&format!(
                "loop: disk-pressure-active: {} bytes free is under the {} byte low watermark; \
                 admitting {} signed Stado release delivery and no ordinary work",
                current_free_bytes.unwrap_or_default(),
                disk_low_bytes.unwrap_or_default(),
                queued.len()
            ));
        }
        let mut started = 0i64;
        let mut diag_vram_rejected = 0i64;
        let mut diag_eligibility_rejected = 0i64;
        let mut diag_eligible = 0i64;
        let mut diag_claim_errors = 0i64;
        // These keys describe one completed scan. The previous scan was
        // already published before this point; carrying its last error into a
        // later clean scan would turn a historical refusal into current state.
        for key in [
            "last_claim_error_job",
            "last_claim_error",
            "last_claim_error_at",
        ] {
            agent_diag.remove(key);
        }
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
        // Per-card budget for this tick, emptiest first. The driver's frees
        // already include every allocation that exists; the subtraction below
        // covers the window in which a job this tick admitted has not allocated
        // yet, which is exactly when a second claim would otherwise be sized
        // against memory the first one is about to take.
        let mut card_budget: Vec<(String, i64)> = cards
            .iter()
            .map(|card| (card.uuid.clone(), card.free_vram_gb))
            .collect();
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
            if let Some(reason) = helpers::eligibility_refusal(
                job,
                &gpu_type,
                total_vram_gb,
                kind,
                &consumer_id,
                slots.len(),
                pinned_only,
            ) {
                diag_eligibility_rejected += 1;
                // The count alone read `eligibility_rejected=72,
                // eligible_count=0` on the always-on mac for seven days and
                // named none of the nine rules that produce it, so the number
                // could not distinguish a wrong pin from a wrong platform and
                // an operator holding it had to reconstruct the answer from the
                // host's process table. The newest refusal carries its rule and
                // the job it judged.
                agent_diag.insert(
                    "last_eligibility_reject_job_id".into(),
                    Value::from(job.job_id.clone()),
                );
                agent_diag.insert("last_eligibility_reject_reason".into(), Value::from(reason));
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
            // Which board. A job that deliberately shares the GPU joins the
            // board its co-tenant is already on -- sharing means one card, not
            // "any card" -- and everything else takes the emptiest board that
            // can hold it. One card, or none, means nothing to choose and the
            // job keeps the driver's default.
            let shared_uuid = if is_raw_share {
                slots
                    .iter()
                    .find(|s| activation_extraction_must_share_gpu(&s.slot.job.command))
                    .and_then(|s| s.gpu_uuid.clone())
            } else {
                None
            };
            let placement = if card_budget.len() < 2 {
                None
            } else if let Some(uuid) = shared_uuid {
                Some(uuid)
            } else {
                card_budget
                    .iter()
                    .filter(|(_, free)| *free >= need)
                    .max_by_key(|(_, free)| *free)
                    .map(|(uuid, _)| uuid.clone())
            };
            let new_slot = match super::slots::start_slot(
                &store,
                job.clone(),
                &hostname,
                log_fn,
                kind,
                placement.as_deref(),
            )
            .await
            {
                Ok(slot) => slot,
                Err(super::slots::StartSlotError::Claim(exc)) => {
                    // One job's claim is that job's problem. Returning here
                    // ends the tick, and `cli::agent` restarts the whole loop:
                    // on charless-mac-mini a single queued job whose durable
                    // transition record could not be verified killed the loop
                    // every few seconds for hours, so the nine other queued
                    // jobs were never reached, the census keys never survived
                    // a publish, and every gate read the host as healthy. The
                    // same doctrine `cli::doctor` states for probes holds here:
                    // one failure names itself and the scan continues.
                    disk_cleanup::release_workload_lock(workload_lock, log_fn);
                    log_fn(&format!(
                        "claim refused for {}: {}; skipping this job and continuing the scan",
                        job.job_id, exc
                    ));
                    diag_claim_errors += 1;
                    agent_diag.insert(
                        "last_claim_error_job".into(),
                        Value::from(job.job_id.clone()),
                    );
                    agent_diag.insert("last_claim_error".into(), Value::from(exc.to_string()));
                    agent_diag.insert(
                        "last_claim_error_at".into(),
                        Value::from(isoformat_utc(Utc::now())),
                    );
                    continue;
                }
                Err(super::slots::StartSlotError::Other(exc)) => {
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
            if let Some(uuid) = &placement {
                if let Some(entry) = card_budget.iter_mut().find(|(id, _)| id == uuid) {
                    entry.1 = (entry.1 - need).max(0);
                }
            }
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
        agent_diag.insert("claim_errors".into(), Value::from(diag_claim_errors));
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
