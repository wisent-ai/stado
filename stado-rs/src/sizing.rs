//! Fleet-learned per-model GPU sizing — purely from MEASURED peaks.
//!
//! Port of `stado/sizing/__init__.py`.
//!
//! For a model, the SMALLEST per-GPU peak_vram_gb observed across its
//! SUCCESSFUL per-GPU-probe completions. No formula, no per-model constant,
//! no minimum-sample gate, no hardcoded cap: a single real measurement is
//! the truth and is used immediately.
//!
//! min (not max/mean) because activation-extraction is memory-ELASTIC: it
//! opportunistically grows to fill whatever VRAM the card has, so the same
//! model measures ~89 GiB on a 96 GiB box but completes fine using ~50-74
//! on an 80 GiB card. A run that COMPLETED at peak P is proof the workload
//! fits in P; the smallest such P is the demonstrated-sufficient footprint.
//! Taking max instead let the single largest-GPU sample (89) fence the
//! whole smaller-GPU fleet off the model and re-stall the queue, even
//! though every 80 GiB run finished. gpu_mem_gb gates scheduling
//! eligibility, not the process, and exclusive models get the whole card,
//! so sizing at the smallest proven-sufficient peak safely widens
//! eligibility without changing what the process actually allocates.
//!
//! If a model has ZERO measured completions, observed_vram_gb returns None
//! and the caller does NOT fabricate a number — the job starts on the
//! smallest GPU tier and escalates up the hardware ladder on OOM until it
//! runs, at which point its real peak is measured and every later job of
//! that model is sized from that measurement. There is no hardcoded VRAM
//! guess anywhere in this path; the only inputs are measured peaks and
//! hardware GPU-class capacities.
//!
//! completed/ is thousands of blobs; building the per-model map on every
//! estimate call would blow the tick budget, so it is built once and
//! cached in process for [`OBSERVED_MAP_TTL_S`], the same amortization
//! makespan history and the reaper completion-ref scan already use.
//!
//! Python keeps the caches as module globals; here they live on the
//! [`Sizing`] struct (async storage access needs an owner) and the
//! process-wide instance behind [`global()`] reproduces the module-global
//! semantics for CLI/coordinator one-shot callers. Tests construct their
//! own `Sizing::new()` so caches never leak between cases.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::constants;
use crate::models::{job_state, Job};
use crate::queue::{JobStorage, StorageError};

static MODEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--model\s+(\S+)").expect("static regex compiles"));
static OOM_PROC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)this process has ([0-9.]+) GiB memory in use").expect("static regex compiles")
});
static OOM_ALLOC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Tried to allocate ([0-9.]+) (MiB|GiB)").expect("static regex compiles")
});
static OOM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)out of memory|OutOfMemoryError|CUDA error: out of memory|\
         CUDA_ERROR_OUT_OF_MEMORY|cuBLAS.*alloc|cudaErrorMemoryAllocation",
    )
    .expect("static regex compiles")
});

/// Python `_COMPLETED_SAMPLE_CAP = _wc.COMPLETED_SAMPLE_CAP`.
const COMPLETED_SAMPLE_CAP: usize = constants::COMPLETED_SAMPLE_CAP;
/// Python `_TTL_S = _wc.OBSERVED_MAP_TTL_S`.
const OBSERVED_MAP_TTL_S: u64 = constants::OBSERVED_MAP_TTL_S;
/// Agent-liveness window: a capacity broadcast older than this means the
/// agent is gone, so its GPU is not part of "the actual fleet" right now.
/// This is a staleness threshold, not a VRAM figure.
/// Python `_LIVE_TTL_S = _wc.LIVE_CAPACITY_TTL_S`.
const LIVE_TTL_S: i64 = constants::LIVE_CAPACITY_TTL_S as i64;
/// Python caches `_live_total_vrams` for 30s so the agent claim loop /
/// submit path does not relist every call.
const CAPS_CACHE_TTL_S: u64 = 30;

/// Python `_model_of`: `--model <value>` out of a command line,
/// quote-stripped. "" when absent.
pub fn model_of(command: &str) -> String {
    let Some(caps) = MODEL_RE.captures(command) else {
        return String::new();
    };
    caps[1].trim_matches(['\'', '"']).to_string()
}

/// Python `_oom_required_gb`: VRAM the OOMing process provably needed,
/// from the PyTorch OOM message. 0 when the message carries no
/// "this process has ..." figure.
pub fn oom_required_gb(text: &str) -> i64 {
    let proc = OOM_PROC_RE.captures(text);
    let alloc = OOM_ALLOC_RE.captures(text);
    let Some(proc) = proc else { return 0 };
    let mut need: f64 = proc[1].parse().unwrap_or(0.0);
    if let Some(alloc) = alloc {
        let x: f64 = alloc[1].parse().unwrap_or(0.0);
        need += if alloc[2].eq_ignore_ascii_case("gib") {
            x
        } else {
            x / 1024.0
        };
    }
    (need.ceil() as i64).max(1)
}

/// Python `_OOM_RE.search`.
pub fn is_oom_error(text: &str) -> bool {
    OOM_RE.is_match(text)
}

/// In-process caches for the observed-VRAM map and the live-capacity
/// ladder. Cheap to construct; clone via [`global()`] for the process-wide
/// instance.
pub struct Sizing {
    observed: Mutex<ObservedCache>,
    caps: Mutex<CapsCache>,
}

#[derive(Default)]
struct ObservedCache {
    map: Option<HashMap<String, i64>>,
    built_at: Option<Instant>,
}

#[derive(Default)]
struct CapsCache {
    vrams: Option<Vec<i64>>,
    built_at: Option<Instant>,
}

impl Default for Sizing {
    fn default() -> Self {
        Self::new()
    }
}

impl Sizing {
    pub fn new() -> Self {
        Self {
            observed: Mutex::new(ObservedCache {
                map: None,
                built_at: None,
            }),
            caps: Mutex::new(CapsCache {
                vrams: None,
                built_at: None,
            }),
        }
    }

    /// Smallest demonstrated-sufficient MEASURED peak_vram_gb for `model`
    /// (min over its successful per-GPU-probe completions), or None if the
    /// model has no such measured completion yet (caller must NOT fabricate
    /// a number — start on the smallest ACTUAL fleet GPU and escalate via
    /// live capacities). Python `observed_vram_gb`.
    pub async fn observed_vram_gb(
        &self,
        store: &JobStorage,
        model: &str,
    ) -> Result<Option<i64>, StorageError> {
        let mut cache = self.observed.lock().await;
        let fresh = cache.map.is_some()
            && cache
                .built_at
                .is_some_and(|t| t.elapsed() <= Duration::from_secs(OBSERVED_MAP_TTL_S));
        if !fresh {
            // A hard failure propagates; the cache keeps the last good map
            // until a later rebuild succeeds (see build_observed_map).
            let map = self.build_observed_map(store).await?;
            cache.map = Some(map);
            cache.built_at = Some(Instant::now());
        }
        Ok(cache.map.as_ref().and_then(|m| m.get(model).copied()))
    }

    /// Smallest REAL GPU total_vram_gb currently in the fleet, or None if
    /// no live agent is broadcasting (then the caller must not invent a
    /// number — the job stays unsized until a real GPU appears).
    /// Python `smallest_live_vram`.
    pub async fn smallest_live_vram(
        &self,
        store: &JobStorage,
    ) -> Result<Option<i64>, StorageError> {
        Ok(self.live_total_vrams(store).await?.first().copied())
    }

    /// Smallest REAL fleet total_vram_gb strictly greater than `current`,
    /// or None if no live GPU is bigger (genuine ceiling — not a guess).
    /// Python `next_live_vram`.
    pub async fn next_live_vram(
        &self,
        store: &JobStorage,
        current: i64,
    ) -> Result<Option<i64>, StorageError> {
        Ok(self
            .live_total_vrams(store)
            .await?
            .into_iter()
            .find(|v| *v > current))
    }

    /// model -> min measured peak_vram_gb over its completed runs.
    /// Python `_build_observed_map`.
    ///
    /// Any model with >= 1 real measurement is included; there is no
    /// minimum-sample gate. A coordinator-side storage list/read outage
    /// must not silently erase the map fleet-wide, so a hard failure here
    /// propagates; the caller's cache keeps the last good map until a
    /// later rebuild succeeds.
    async fn build_observed_map(
        &self,
        store: &JobStorage,
    ) -> Result<HashMap<String, i64>, StorageError> {
        let completed_paths: Vec<String> = store
            .list_paths("completed/", 0)
            .await?
            .into_iter()
            .take(COMPLETED_SAMPLE_CAP)
            .collect();
        let mut peaks: HashMap<String, Vec<i64>> = HashMap::new();
        if !completed_paths.is_empty() {
            for text in download_many(store, &completed_paths)
                .await?
                .into_iter()
                .flatten()
            {
                let doc: Value = serde_json::from_str(&text)?;
                if doc.get("state").and_then(Value::as_str) != Some("completed") {
                    continue;
                }
                // Python `isinstance(peak, int)`: a JSON float (74.0) is not
                // an int and as_i64 rejects it the same way.
                let Some(peak) = doc.get("peak_vram_gb").and_then(Value::as_i64) else {
                    continue;
                };
                if peak <= 0 {
                    continue; // unmeasured / CPU job — not a usable observation
                }
                if doc.get("peak_vram_per_gpu") != Some(&Value::Bool(true)) {
                    // Legacy record from the pre-0.4.241 probe that summed
                    // used_memory ACROSS GPUs (cross-GPU total, not per-card).
                    // Mixing those into the per-model sample set corrupts the
                    // signal, so only peaks the corrected per-GPU probe produced
                    // are trusted. Until a model has at least one such record
                    // observed_vram_gb returns None and the job sizes via the
                    // smallest-live-GPU + OOM-escalate path (no fabricated
                    // number).
                    continue;
                }
                let model = model_of(doc.get("command").and_then(Value::as_str).unwrap_or(""));
                if model.is_empty() {
                    continue;
                }
                peaks.entry(model).or_default().push(peak);
            }
        }

        // A per_gpu=true peak larger than the smallest live-fleet GPU came
        // from a bigger card running this memory-elastic workload (grows to
        // fill VRAM); it is not a valid lower bound for a fleet GPU and
        // fences the model off the whole smaller fleet. Drop it; if none
        // remain the model is unmeasured (observed->None) so it sizes via
        // smallest-live-GPU+escalate, runs, and yields a fleet-representative
        // sample that then governs via min() -> min-agg self-bootstraps
        // (gpt-oss-20b 89 on 96GB box vs 50-74 on 80GB, 2026-05-19).
        let smallest_live = self.smallest_live_vram(store).await?;
        let mut out: HashMap<String, i64> = HashMap::new();
        for (model, samples) in peaks {
            let usable: Vec<i64> = samples
                .into_iter()
                .filter(|p| smallest_live.is_none_or(|sl| *p <= sl))
                .collect();
            if let Some(min) = usable.iter().min() {
                out.insert(model, *min);
            }
        }

        let failed_paths: Vec<String> = store
            .list_paths("failed/", 0)
            .await?
            .into_iter()
            .take(COMPLETED_SAMPLE_CAP)
            .collect();
        if !failed_paths.is_empty() {
            let live_vrams = self.live_total_vrams(store).await?;
            let max_live_vram = live_vrams.last().copied();
            let mut floors: HashMap<String, i64> = HashMap::new();
            for text in download_many(store, &failed_paths)
                .await?
                .into_iter()
                .flatten()
            {
                let doc: Value = serde_json::from_str(&text)?;
                let model = model_of(doc.get("command").and_then(Value::as_str).unwrap_or(""));
                if model.is_empty() {
                    continue;
                }
                let floor = oom_required_gb(doc.get("error").and_then(Value::as_str).unwrap_or(""));
                if max_live_vram.is_some_and(|mx| floor > mx) {
                    continue;
                }
                if floor > *floors.get(&model).unwrap_or(&0) {
                    floors.insert(model, floor);
                }
            }
            for (model, floor) in floors {
                let entry = out.entry(model).or_insert(0);
                *entry = (*entry).max(floor);
            }
        }
        Ok(out)
    }

    /// Ascending, de-duplicated list of the REAL total_vram_gb values the
    /// fleet is broadcasting right now — i.e. the actual GPUs that exist,
    /// read from <bucket>/capacity/ (each agent publishes its own
    /// nvidia-smi total_vram_gb). No catalog, no hand-written tier list.
    /// Stale broadcasts (older than [`LIVE_TTL_S`]) are excluded. Cached
    /// 30s so the agent claim loop / submit path does not relist every
    /// call. Python `_live_total_vrams`.
    pub async fn live_total_vrams(&self, store: &JobStorage) -> Result<Vec<i64>, StorageError> {
        let mut cache = self.caps.lock().await;
        if let Some(vrams) = &cache.vrams {
            if cache
                .built_at
                .is_some_and(|t| t.elapsed() < Duration::from_secs(CAPS_CACHE_TTL_S))
            {
                return Ok(vrams.clone());
            }
        }
        let now = Utc::now();
        let mut vrams: Vec<i64> = Vec::new();
        let paths = store.list_paths("capacity/", 0).await?;
        for text in download_many(store, &paths).await?.into_iter().flatten() {
            let doc: Value = serde_json::from_str(&text)?;
            let Some(pub_at) = doc.get("published_at").and_then(Value::as_str) else {
                continue;
            };
            // Python `except Exception: continue` — an unparseable
            // published_at just drops the broadcast.
            let Ok(published) = DateTime::parse_from_rfc3339(pub_at) else {
                continue;
            };
            let age = (now - published.with_timezone(&Utc)).num_seconds();
            if age > LIVE_TTL_S {
                continue;
            }
            if let Some(tv) = doc.get("total_vram_gb").and_then(Value::as_i64) {
                if tv > 0 && !vrams.contains(&tv) {
                    vrams.push(tv);
                }
            }
        }
        vrams.sort_unstable();
        cache.vrams = Some(vrams.clone());
        cache.built_at = Some(Instant::now());
        Ok(vrams)
    }

    /// A job that OOMed while sized at some GPU — and has NO measured
    /// peak yet — is moved to the next-larger REAL GPU currently in the
    /// fleet and requeued (running -> queue) instead of failed. Returns
    /// true iff requeued. If no live GPU is larger, or the model already
    /// has a measured peak, this does nothing and the caller fails it.
    ///
    /// No hand-written tier ladder: the next size comes from the actual
    /// GPUs the fleet is broadcasting (next_live_vram). An unmeasured model
    /// starts on the smallest real fleet GPU and climbs the real observed
    /// GPUs one OOM at a time until it runs; that run's measured nvidia-smi
    /// peak then sizes every later job of the model.
    /// Python `escalate_on_oom`.
    pub async fn escalate_on_oom(
        &self,
        store: &JobStorage,
        job: &mut Job,
        error_text: &str,
    ) -> Result<bool, StorageError> {
        if !is_oom_error(error_text) {
            return Ok(false);
        }
        let model = model_of(&job.command);
        if model.is_empty() {
            return Ok(false);
        }
        let cur = job.gpu_mem_gb;
        let measured_floor = oom_required_gb(error_text);
        let nxt = if measured_floor > cur {
            let live_vrams = self.live_total_vrams(store).await?;
            if !live_vrams.is_empty() && measured_floor > *live_vrams.last().unwrap_or(&0) {
                return Ok(false);
            }
            Some(measured_floor)
        } else {
            if self.observed_vram_gb(store, &model).await?.is_some() {
                return Ok(false); // measured already; a real OOM is a real failure
            }
            self.next_live_vram(store, cur).await?
        };
        let Some(nxt) = nxt else {
            return Ok(false); // no live GPU bigger than current — genuine failure
        };
        job.gpu_mem_gb = nxt;
        job.state = job_state::QUEUED.into();
        job.failed_at = None;
        job.error = None;
        job.instance_ref = None;
        job.started_at = None;
        store.move_job(job, "running", "queue").await?;
        store.cleanup_status(&job.job_id).await?;
        Ok(true)
    }

    /// Coordinator-authoritative sizing pass, run once per tick BEFORE
    /// assignment. Python `normalize_queue_sizing`.
    ///
    /// A queued job's gpu_mem_gb is owned by the sizing path, not by the
    /// agent that last touched it. An agent still on pre-0.4.237
    /// wisent-compute (not yet drifted) requeues jobs writing the OLD
    /// hardcoded estimate_gpu_memory output (gpt-oss-20b -> 64/12/80); the
    /// 0.4.238 makespan apply-assignment then faithfully PRESERVES that
    /// stale value because it only rewrites assigned_to. So the queue keeps
    /// re-accumulating hardcoded sizes until every agent has drifted.
    ///
    /// This pass closes that gap structurally: for every queued job whose
    /// model has NO measured peak yet, force gpu_mem_gb back to 0 — the
    /// canonical "no stored size, sized live at claim time" state. For a
    /// model WITH a measured peak, stamp that measured peak (the ground
    /// truth). Either way the stored number is never a hardcoded guess.
    /// A lagging agent's stale write is corrected within one tick instead
    /// of persisting until fleet-wide drift completes.
    ///
    /// Fresh read-modify-write of ONLY gpu_mem_gb so a concurrent
    /// makespan assigned_to write on the same blob is not lost. Returns
    /// the number of queue blobs corrected this tick.
    pub async fn normalize_queue_sizing(
        &self,
        store: &JobStorage,
        log_fn: &dyn Fn(&str),
    ) -> Result<usize, StorageError> {
        let mut corrected = 0usize;
        for job in store.list_jobs("queue", 0).await? {
            let model = model_of(&job.command);
            if model.is_empty() {
                continue;
            }
            let peak = self.observed_vram_gb(store, &model).await?;
            let desired = peak.unwrap_or(0);
            if job.gpu_mem_gb == desired {
                continue;
            }
            let Some(mut fresh) = store.read_job("queue", &job.job_id).await? else {
                continue; // claimed/moved since tick start
            };
            if fresh.gpu_mem_gb == desired {
                continue;
            }
            fresh.gpu_mem_gb = desired;
            store.write_job("queue", &fresh).await?;
            corrected += 1;
        }
        if corrected > 0 {
            log_fn(&format!(
                "sizing: normalized {corrected} queue jobs \
                 (unmeasured->0 / measured->peak); stale-agent clobber corrected"
            ));
        }
        Ok(corrected)
    }
}

/// The process-wide cache holder, reproducing Python's module-global
/// `_cache` / `_caps_cache` for one-shot callers (CLI submit, coordinator
/// tick). Tests should build their own [`Sizing::new()`].
pub fn global() -> &'static Sizing {
    static GLOBAL: LazyLock<Sizing> = LazyLock::new(Sizing::new);
    &GLOBAL
}

/// Parallel-download the given blob paths (Python
/// `ThreadPoolExecutor(max_workers=32)` + `pool.map` → `buffered(32)`,
/// path order preserved). A missing blob (TOCTOU: moved between list and
/// download) comes back as None and is skipped by the caller; any other
/// error propagates so a real outage is visible.
async fn download_many(
    store: &JobStorage,
    paths: &[String],
) -> Result<Vec<Option<String>>, StorageError> {
    use futures::StreamExt;
    futures::stream::iter(paths)
        .map(|path| store.download_text(path))
        .buffered(32)
        .collect::<Vec<Result<Option<String>, StorageError>>>()
        .await
        .into_iter()
        .collect()
}

