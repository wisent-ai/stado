//! GPU detection, eligibility, capacity helpers for the local agent loop.
//!
//! Port of `stado/providers/local/helpers/__init__.py`.
//!
//! Extracted from providers/local_agent.py in Python to keep the parent
//! file under the 300-line cap. The 0.4.100 cut adds consumer_id +
//! assigned_to enforcement to _job_eligible so the coordinator's
//! centralized matcher (_assign_jobs_to_agents in coordinator.py) can pin
//! queued jobs to specific agents instead of every agent racing to claim
//! from a global FIFO. Without this enforcement, fleet-aware LPT
//! scheduling collapses to greedy first-come-first-served and the makespan
//! grows.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use crate::catalog::GPU_SIZING;
use crate::config::{self, estimate_gpu_memory};
use crate::constants;
use crate::models::{activation_extraction_must_share_gpu, Job};
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;

// `_accel_hourly_rate` is NOT re-implemented here: the Python docstring
// says it "mirrors scheduler._accel_hourly_rate so both consumers apply the
// same cost-cap rule" — the Rust port shares the single implementation in
// `scheduler::scheduler::accel_hourly_rate` (re-exported for callers that
// imported it from helpers in Python).
pub use crate::scheduler::scheduler::accel_hourly_rate;

use super::gpu_probe;
use super::Slot;

/// Python `VAST_API`.
pub const VAST_API: &str = "https://console.vast.ai/api/v0";

static MODEL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"--model\s+(\S+)").expect("static regex compiles"));

/// Check if any Vast.ai instance is currently rented on this machine.
/// Python `_vast_has_renter`.
///
/// No exception swallow: a failed API call would otherwise be silently
/// treated as 'no renter' and the agent would claim jobs on top of a paid
/// Vast.ai renter, wasting both the renter's GPU time and ours. Caller
/// must crash visibly so the operator notices Vast.ai outage — hence the
/// `Err` propagation (and `error_for_status`, matching urllib's raise on
/// HTTP errors).
pub async fn vast_has_renter() -> anyhow::Result<bool> {
    let api_key = crate::skarbiec::read_string("stado-vast", "api_key")
        .await?
        .unwrap_or_default();
    if api_key.is_empty() {
        return Ok(false);
    }
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{VAST_API}/instances?owner=me"))
        .bearer_auth(api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(has_running_instance(&body))
}

/// Pure: any instance in the Vast.ai /instances payload with
/// `actual_status == "running"`.
pub fn has_running_instance(body: &serde_json::Value) -> bool {
    body.get("instances")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|instances| {
            instances.iter().any(|i| {
                i.get("actual_status").and_then(serde_json::Value::as_str) == Some("running")
            })
        })
}

/// Pure: Python `name.lower().replace(" ", "-").replace("geforce-", "nvidia-")`.
pub fn normalize_gpu_name(name: &str) -> String {
    name.to_lowercase()
        .replace(' ', "-")
        .replace("geforce-", "nvidia-")
}

/// Pure: first line of a `nvidia-smi --format=csv,noheader,nounits` reply
/// as an integer MiB count. None when unparsable (Python ValueError path).
pub fn parse_smi_mib_first(stdout: &str) -> Option<i64> {
    stdout.trim().lines().next()?.trim().parse().ok()
}

/// Python `_detect_gpu_type`: nvidia-smi on Linux, sysctl brand string on
/// macOS, else "cpu".
pub async fn detect_gpu_type() -> String {
    if let Ok(out) = tokio::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
        .await
    {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // Python: r.stdout.strip().split("\n")[0] — an empty result is
            // returned verbatim (""), not treated as "no GPU".
            return normalize_gpu_name(stdout.trim().lines().next().unwrap_or(""));
        }
    }
    if let Ok(out) = tokio::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .await
    {
        if String::from_utf8_lossy(&out.stdout).contains("Apple") {
            return "apple-mps".to_string();
        }
    }
    "cpu".to_string()
}

/// One accelerator, as the driver reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuCard {
    /// Driver UUID (`GPU-...`), which is what `CUDA_VISIBLE_DEVICES` should
    /// carry: an index is positional and reorders with the enumeration mode,
    /// while a UUID names one board.
    pub uuid: String,
    pub total_vram_gb: i64,
    pub free_vram_gb: i64,
}

/// Pure parser for `nvidia-smi --query-gpu=uuid,memory.total,memory.free
/// --format=csv,noheader,nounits`: one [`GpuCard`] per readable row.
pub fn parse_gpu_cards(stdout: &str) -> Vec<GpuCard> {
    let mut cards = Vec::new();
    for line in stdout.lines() {
        let mut fields = line.split(',').map(str::trim);
        let (Some(uuid), Some(total), Some(free)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(total_mib), Ok(free_mib)) = (total.parse::<i64>(), free.parse::<i64>()) else {
            continue;
        };
        if uuid.is_empty() {
            continue;
        }
        cards.push(GpuCard {
            uuid: uuid.to_string(),
            total_vram_gb: total_mib / 1024,
            free_vram_gb: free_mib / 1024,
        });
    }
    cards
}

/// Every accelerator this host has, newest driver reading. Empty when
/// `nvidia-smi` is absent or answers nothing parsable.
///
/// One call, every card: the previous readings took the FIRST line of a
/// per-GPU query, so on the fleet's two-card host the agent measured card 0
/// and nothing else -- it advertised 35 GiB free while a second, idle 95 GiB
/// board sat beside it, and two concurrent slots were both admitted against
/// card 0's numbers.
pub async fn smi_gpu_cards() -> Vec<GpuCard> {
    let Ok(out) = tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=uuid,memory.total,memory.free",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_gpu_cards(&String::from_utf8_lossy(&out.stdout))
}

/// Python `_detect_local_vram_gb`: total VRAM in GB of the largest card this
/// host has, 0 if none.
///
/// The largest single card, not the sum: a job that does not shard can only
/// use one board, and every admission comparison downstream treats this as
/// "the biggest thing that fits".
pub async fn detect_local_vram_gb() -> i64 {
    smi_gpu_cards()
        .await
        .iter()
        .map(|card| card.total_vram_gb)
        .max()
        .unwrap_or(0)
}

/// Python `_smi_free_vram_gb`: live free VRAM in GB on the emptiest card,
/// -1 if the driver is unreadable.
///
/// The emptiest card is the honest answer to "will this job fit": a workload
/// holding card 0 -- ours, an external one like ComfyUI, or a Vast renter -- does
/// not shrink what card 1 can take.
pub async fn smi_free_vram_gb() -> i64 {
    smi_gpu_cards()
        .await
        .iter()
        .map(|card| card.free_vram_gb)
        .max()
        .unwrap_or(-1)
}

/// Every GCP gpu_type whose required VRAM tier <= local VRAM.
/// Python `_compat_accel_types`.
pub fn compat_accel_types(local_vram_gb: i64) -> Vec<String> {
    let mut accels: Vec<String> = Vec::new();
    if let Some(sizing) = GPU_SIZING.get(crate::capabilities::ProviderId::Gcp.as_str()) {
        // BTreeMap iterates tiers ascending, matching Python's sorted(...).
        for (tier, (_, accel)) in sizing {
            if local_vram_gb >= *tier && !accel.is_empty() && !accels.iter().any(|a| a == accel) {
                accels.push((*accel).to_string());
            }
        }
    }
    accels
}

/// Local-agent claim rules. Python `_job_eligible`.
///
/// NEW (0.4.100): if job.assigned_to was set by the centralized
/// coordinator matcher, only the agent whose consumer_id matches may
/// claim. Empty assigned_to means unassigned and any-eligible-agent may
/// claim (pre-0.4.100 back-compat).
///
/// NEW (0.4.131): job.exclusive=True is only eligible on an agent with
/// zero active slots. Caller passes its current slot count via
/// active_slot_count so this filter runs at the agent-side claim loop
/// without needing the slot dict here.
///
/// NEW (0.4.379): job.pinned_host is an operator hard-pin from submit
/// time; only the named consumer may ever claim it. pinned_only=True
/// (registry target flag) reverses the default: this agent then claims
/// ONLY jobs explicitly routed to it (pinned_host or assigned_to), so a
/// shared workstation never picks up stray queue backlog. Pin matching is
/// case-insensitive: registry hostnames are stored normalized while
/// consumer_id carries the machine's verbatim gethostname() casing.
#[allow(clippy::too_many_arguments)]
pub fn job_eligible(
    job: &Job,
    gpu_type: &str,
    vram_gb: i64,
    kind: &str,
    consumer_id: &str,
    active_slot_count: usize,
    pinned_only: bool,
) -> bool {
    let pinned_host = job.pinned_host.to_lowercase();
    let cid = consumer_id.to_lowercase();
    if !pinned_host.is_empty() && pinned_host != cid {
        return false;
    }
    let assigned = job.assigned_to.as_str();
    if !assigned.is_empty() && !consumer_id.is_empty() && assigned != consumer_id {
        return false;
    }
    if pinned_only && pinned_host != cid && assigned != consumer_id {
        return false;
    }
    if job.exclusive && active_slot_count > 0 {
        return false;
    }
    if crate::capabilities::execution_adapter(kind)
        != Some(crate::capabilities::ExecutionAdapter::Local)
    {
        if let Some(caps) = MODEL_RE.captures(&job.command) {
            if config::is_local_only_model(caps[1].trim_matches(['\'', '"'])) {
                return false;
            }
        }
    }
    if job.pin_to_provider
        && !crate::capabilities::same_variant(
            crate::capabilities::RuntimeFacet::Execution,
            &job.provider,
            kind,
        )
    {
        return false;
    }
    let job_accel = job.gpu_type.as_str();
    let matches = crate::capabilities::ProviderId::Local.matches(&job.provider)
        || job_accel.is_empty()
        || job_accel == gpu_type
        || (vram_gb > 0 && compat_accel_types(vram_gb).iter().any(|a| a == job_accel));
    if !matches {
        return false;
    }
    let cap = job.max_cost_per_hour_usd;
    if cap > 0.0 && !job_accel.is_empty() {
        let rate = accel_hourly_rate(job_accel, job.preemptible);
        if rate > 0.0 && rate > cap {
            return false;
        }
    }
    true
}

/// Slot-shaped capacity broadcast for back-compat schedulers.
/// Python `_build_capacity_dict`.
pub fn build_capacity_dict(
    gpu_type: &str,
    free_vram_gb: i64,
    total_vram_gb: i64,
) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    if gpu_type.is_empty() || gpu_type == "cpu" || free_vram_gb <= 0 {
        return out;
    }
    if let Some(sizing) = GPU_SIZING.get(crate::capabilities::ProviderId::Gcp.as_str()) {
        for (tier, (_, accel)) in sizing {
            if total_vram_gb >= *tier && !accel.is_empty() {
                let n = (free_vram_gb / (*tier).max(1)).max(0);
                if n > 0 {
                    let entry = out.entry((*accel).to_string()).or_insert(0);
                    *entry = (*entry).max(n);
                }
            }
        }
    }
    if !out.contains_key(gpu_type) {
        out.insert(gpu_type.to_string(), 1);
    }
    out
}

/// The same broadcast for a host with more than one card: how many slots of
/// each tier fit across all of them, and one entry for this host's own
/// gpu_type.
///
/// A tier count is summed per card, never derived from a pooled total. Two
/// 95 GiB boards do not hold a 190 GiB model, and a card something else is busy
/// on does not reduce what its neighbour can take -- the single-pool answer was
/// wrong in both directions at once.
pub fn build_capacity_dict_per_card(
    gpu_type: &str,
    free_vram_gb_per_card: &[i64],
    total_vram_gb: i64,
) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    if gpu_type.is_empty() || gpu_type == "cpu" {
        return out;
    }
    if free_vram_gb_per_card.iter().all(|free| *free <= 0) {
        return out;
    }
    if let Some(sizing) = GPU_SIZING.get(crate::capabilities::ProviderId::Gcp.as_str()) {
        for (tier, (_, accel)) in sizing {
            if total_vram_gb < *tier || accel.is_empty() {
                continue;
            }
            let n: i64 = free_vram_gb_per_card
                .iter()
                .map(|free| (free / (*tier).max(1)).max(0))
                .sum();
            if n > 0 {
                let entry = out.entry((*accel).to_string()).or_insert(0);
                *entry = (*entry).max(n);
            }
        }
    }
    if !out.contains_key(gpu_type) {
        let usable = free_vram_gb_per_card
            .iter()
            .filter(|free| **free > 0)
            .count() as i64;
        out.insert(gpu_type.to_string(), usable.max(1));
    }
    out
}

/// Python `_slot_is_exclusive`.
pub fn slot_is_exclusive(slot: &Slot) -> bool {
    if activation_extraction_must_share_gpu(&slot.job.command) {
        return false;
    }
    // Per-job opt-in: Job.exclusive=True takes precedence over the
    // regex-on-command path. Used for workloads (e.g. Z-Image LoRA
    // training, SDXL full finetune) whose peak VRAM is hard to bound from
    // the command string alone, but which the submitter has tagged
    // exclusive at submit time.
    if slot.job.exclusive {
        return true;
    }
    match MODEL_RE.captures(&slot.job.command) {
        Some(caps) => config::is_exclusive_model(caps[1].trim_matches(['\'', '"'])),
        None => false,
    }
}

/// Best known VRAM footprint for a running slot. Python `_slot_vram`.
///
/// Prefer live per-process nvidia-smi attribution when available. Fall
/// back to the declared/observed model estimate only before the job has
/// allocated CUDA memory. This keeps admission tied to measured live usage
/// instead of a stale pre-start estimate.
pub async fn slot_vram(
    slot: &Slot,
    sizing: &Sizing,
    store: &JobStorage,
) -> Result<i64, StorageError> {
    let declared = slot
        .job
        .gpu_mem_gb
        .max(estimate_gpu_memory(&slot.job.command, sizing, store).await?);
    let live = slot_live_vram_gb(slot).await;
    Ok(declared.max(live).max(slot.peak_vram_gb))
}

/// Python `_slot_live_vram_gb`: 0 when the slot has no pid or the probe is
/// unreadable (Python's broad `except Exception` / `max(0, ...)`).
pub async fn slot_live_vram_gb(slot: &Slot) -> i64 {
    let Some(pid) = slot.pid else { return 0 };
    gpu_probe::smi_job_used_gb(pid).await.max(0)
}

/// `kill(pid, 0)` liveness check, standing in for Python's
/// `proc.poll() is None`.
pub(crate) fn pid_alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

/// True when a GPU slot is live but CUDA allocation is not visible yet.
/// Python `_slot_waiting_for_vram`.
pub async fn slot_waiting_for_vram(
    slot: &Slot,
    sizing: &Sizing,
    store: &JobStorage,
) -> Result<bool, StorageError> {
    let Some(pid) = slot.pid else {
        return Ok(false);
    };
    if !pid_alive(pid) {
        return Ok(false);
    }
    let declared = slot
        .job
        .gpu_mem_gb
        .max(estimate_gpu_memory(&slot.job.command, sizing, store).await?);
    Ok(declared > 0 && slot_live_vram_gb(slot).await <= 0)
}

/// Pure parser for /proc/meminfo: value of `key` (e.g. "MemAvailable:")
/// in GB. None when the key is absent or unparsable.
pub fn parse_meminfo_gb(text: &str, key: &str) -> Option<f64> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb as f64 / (1024.0 * 1024.0));
        }
    }
    None
}

fn meminfo_gb(key: &str) -> f64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| parse_meminfo_gb(&text, key))
        .unwrap_or(-1.0)
}

/// Free host RAM (GB) from /proc/meminfo MemAvailable; -1.0 if unreadable
/// (e.g. non-Linux) — caller treats <0 as 'unknown, do not gate'.
/// Python `_free_ram_gb`.
pub fn free_ram_gb() -> f64 {
    meminfo_gb("MemAvailable:")
}

/// Total host RAM (GB) from /proc/meminfo MemTotal; -1.0 if unreadable.
/// The sum-based admission gate bounds anonymous slot RSS against THIS,
/// not MemAvailable — MemAvailable counts reclaimable staging page-cache
/// as free, so it masked the real footprint and the agent over-admitted to
/// a status=1 OOM at ~100G on a 123G box. Python `_total_ram_gb`.
pub fn total_ram_gb() -> f64 {
    meminfo_gb("MemTotal:")
}

/// Pure parser for /proc/<pid>/status: first VmRSS value in kB.
pub fn parse_status_vmrss_kb(text: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// Sum of non-wisent, non-slot resident RAM (GB). Python
/// `_static_ram_reserve_gb`.
///
/// Walks /proc/*/status and adds VmRSS for every process that is NOT the
/// agent itself and does not look like an extraction slot
/// (extract_and_upload, upload_worker) or the agent binary. This captures
/// ComfyUI, system daemons, and any other baseline load without hardcoding
/// a reserve number.
pub fn static_ram_reserve_gb() -> f64 {
    static_ram_reserve_gb_at(Path::new("/proc"))
}

/// [`static_ram_reserve_gb`] with an injectable procfs root (tests use a
/// fabricated tree under a TempDir).
pub fn static_ram_reserve_gb_at(proc_root: &Path) -> f64 {
    let own = std::process::id();
    let mut total_kb: u64 = 0;
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return 0.0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if pid == own {
            continue;
        }
        let Ok(cmd) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let cmd: Vec<u8> = cmd
            .iter()
            .map(|b| if *b == 0 { b' ' } else { *b })
            .collect();
        // Skip the agent binary, extraction slots, and their upload workers.
        if [
            b"wc agent".as_slice(),
            b"extract_and_upload",
            b"upload_worker",
        ]
        .iter()
        .any(|token| contains_subslice(&cmd, token))
        {
            continue;
        }
        if let Some(kb) = std::fs::read_to_string(entry.path().join("status"))
            .ok()
            .and_then(|text| parse_status_vmrss_kb(&text))
        {
            total_kb += kb;
        }
    }
    total_kb as f64 / (1024.0 * 1024.0)
}

/// Dynamic RAM headroom: 5% of total RAM with a 4 GiB floor.
/// Python `_ram_safety_buffer_gb`.
pub fn ram_safety_buffer_gb() -> f64 {
    let total = total_ram_gb();
    if total <= 0.0 {
        return constants::RAM_SAFETY_BUFFER_MIN_GB as f64;
    }
    (constants::RAM_SAFETY_BUFFER_MIN_GB as f64).max(total * constants::RAM_SAFETY_BUFFER_FRACTION)
}

/// Pure: summed VmRSS of `pids` under `proc_root`, in GB.
pub fn sum_rss_gb(proc_root: &Path, pids: &std::collections::HashSet<i32>) -> f64 {
    let mut total_kb: u64 = 0;
    for p in pids {
        if let Some(kb) = std::fs::read_to_string(proc_root.join(p.to_string()).join("status"))
            .ok()
            .and_then(|text| parse_status_vmrss_kb(&text))
        {
            total_kb += kb;
        }
    }
    total_kb as f64 / (1024.0 * 1024.0)
}

/// Measured resident host RAM (GB) of a running slot's whole process tree
/// (bash + python + upload workers), summed from /proc/<pid>/status VmRSS.
/// This is the OBSERVED per-job footprint used to decide if another job
/// fits — no hardcoded estimate. Python `_slot_rss`.
pub async fn slot_rss(slot: &Slot) -> f64 {
    let Some(pid) = slot.pid else { return 0.0 };
    let pids = gpu_probe::proc_tree_pids(pid).await;
    sum_rss_gb(Path::new("/proc"), &pids)
}

/// True when no queued job fits + is eligible for this consumer.
/// Python `_no_eligible_in_queue`.
#[allow(clippy::too_many_arguments)]
pub async fn no_eligible_in_queue(
    store: &JobStorage,
    sizing: &Sizing,
    gpu_type: &str,
    total_vram_gb: i64,
    free_vram_gb: i64,
    kind: &str,
    consumer_id: &str,
    active_slot_count: usize,
) -> Result<bool, StorageError> {
    let queued = store.list_jobs("queue", 0).await?;
    for job in queued {
        let need = job
            .gpu_mem_gb
            .max(estimate_gpu_memory(&job.command, sizing, store).await?);
        if need > free_vram_gb {
            continue;
        }
        if !job_eligible(
            &job,
            gpu_type,
            total_vram_gb,
            kind,
            consumer_id,
            active_slot_count,
            false,
        ) {
            continue;
        }
        return Ok(false);
    }
    Ok(true)
}

/// Total size of files under d in GB. 0 if dir missing.
/// Python `_staging_size_gb`.
pub fn staging_size_gb(d: &Path) -> f64 {
    if !d.is_dir() {
        return 0.0;
    }
    dir_size_bytes(d) as f64 / 1024f64.powi(3)
}

fn dir_size_bytes(d: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(d) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size_bytes(&path);
        } else if let Ok(md) = path.metadata() {
            total += md.len();
        }
    }
    total
}
