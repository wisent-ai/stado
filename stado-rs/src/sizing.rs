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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use crate::queue::python_json_dumps;
    use serde_json::json;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    /// A capacity/ broadcast like `queue::capacity::publish_capacity` writes.
    async fn publish(store: &JobStorage, cid: &str, total_vram_gb: i64, published_at: &str) {
        let body = python_json_dumps(&json!({
            "consumer_id": cid,
            "kind": "local",
            "free_slots": {"nvidia-l4": 1},
            "published_at": published_at,
            "free_vram_gb": total_vram_gb,
            "total_vram_gb": total_vram_gb,
        }))
        .unwrap();
        store
            .upload_text(&format!("capacity/{cid}.json"), &body)
            .await
            .unwrap();
    }

    fn now_rfc3339() -> String {
        Utc::now().to_rfc3339()
    }

    /// A completed/ blob body with a measured peak.
    fn completed_doc(model: &str, peak: i64, per_gpu: bool) -> String {
        python_json_dumps(&json!({
            "job_id": "c1",
            "command": format!("python -m wisent.scripts.activations.raw.extract_and_upload --model {model} --task t1"),
            "state": "completed",
            "peak_vram_gb": peak,
            "peak_vram_per_gpu": per_gpu,
        }))
        .unwrap()
    }

    #[test]
    fn model_of_extracts_and_strips_quotes() {
        assert_eq!(model_of("run --model org/name-7b --task x"), "org/name-7b");
        assert_eq!(model_of("run --model 'org/quoted' --task x"), "org/quoted");
        assert_eq!(model_of("run --model \"org/dq\""), "org/dq");
        assert_eq!(model_of("no model here"), "");
    }

    #[test]
    fn oom_required_gb_parses_pytorch_message() {
        assert_eq!(oom_required_gb("no figures here"), 0);
        // proc only: ceil(70.2) = 71.
        assert_eq!(
            oom_required_gb("this process has 70.2 GiB memory in use"),
            71
        );
        // proc + GiB alloc.
        assert_eq!(
            oom_required_gb("CUDA out of memory. Tried to allocate 2.00 GiB ... this process has 70.00 GiB memory in use"),
            72
        );
        // proc + MiB alloc: 512 MiB = 0.5 GiB -> ceil(70.5) = 71.
        assert_eq!(
            oom_required_gb("Tried to allocate 512 MiB; this process has 70.00 GiB memory in use"),
            71
        );
        // Case-insensitive, floor of 1.
        assert_eq!(oom_required_gb("THIS PROCESS HAS 0.1 GIB MEMORY IN USE"), 1);
    }

    #[test]
    fn is_oom_error_matches_cuda_variants() {
        assert!(is_oom_error("RuntimeError: CUDA out of memory"));
        assert!(is_oom_error(
            "torch.cuda.OutOfMemoryError: CUDA out of memory"
        ));
        assert!(is_oom_error("cudaErrorMemoryAllocation"));
        assert!(!is_oom_error("disk full"));
    }

    #[tokio::test]
    async fn live_ladder_from_fabricated_capacity_blobs() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        let now = now_rfc3339();
        publish(&store, "local-a", 80, &now).await;
        publish(&store, "local-b", 24, &now).await;
        publish(&store, "local-c", 96, &now).await;
        // Duplicate total + stale broadcast are excluded.
        publish(&store, "local-dup", 80, &now).await;
        let stale = (Utc::now() - chrono::Duration::seconds(LIVE_TTL_S + 60)).to_rfc3339();
        publish(&store, "local-dead", 16, &stale).await;
        // Unparseable published_at is dropped, like Python `except: continue`.
        store
            .upload_text(
                "capacity/broken.json",
                &python_json_dumps(&json!({"consumer_id": "broken", "published_at": "not-a-date", "total_vram_gb": 8})).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            sizing.live_total_vrams(&store).await.unwrap(),
            vec![24, 80, 96]
        );
        assert_eq!(sizing.smallest_live_vram(&store).await.unwrap(), Some(24));
        assert_eq!(sizing.next_live_vram(&store, 24).await.unwrap(), Some(80));
        assert_eq!(sizing.next_live_vram(&store, 96).await.unwrap(), None);
    }

    #[tokio::test]
    async fn no_live_fleet_means_no_fabricated_size() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        assert_eq!(sizing.smallest_live_vram(&store).await.unwrap(), None);
        assert_eq!(sizing.next_live_vram(&store, 24).await.unwrap(), None);
    }

    #[tokio::test]
    async fn observed_vram_is_min_over_trusted_per_gpu_samples() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        publish(&store, "local-a", 80, &now_rfc3339()).await;
        // Trusted per-GPU-probe samples: 74 and 50 -> min = 50.
        store
            .upload_text("completed/a.json", &completed_doc("m1", 74, true))
            .await
            .unwrap();
        store
            .upload_text("completed/b.json", &completed_doc("m1", 50, true))
            .await
            .unwrap();
        // Legacy cross-GPU-sum record (per_gpu=false): NOT trusted.
        store
            .upload_text("completed/c.json", &completed_doc("m1", 89, false))
            .await
            .unwrap();
        // Unmeasured / CPU job: not a usable observation.
        store
            .upload_text("completed/d.json", &completed_doc("m1", 0, true))
            .await
            .unwrap();
        // peak_vram_per_gpu missing entirely: pre-0.4.241 record, skipped.
        let legacy = python_json_dumps(&json!({
            "job_id": "e", "command": "x --model m1 --task t", "state": "completed", "peak_vram_gb": 30,
        }))
        .unwrap();
        store
            .upload_text("completed/e.json", &legacy)
            .await
            .unwrap();

        assert_eq!(
            sizing.observed_vram_gb(&store, "m1").await.unwrap(),
            Some(50)
        );
        assert_eq!(
            sizing
                .observed_vram_gb(&store, "unknown-model")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn only_legacy_records_mean_unmeasured() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        publish(&store, "local-a", 80, &now_rfc3339()).await;
        store
            .upload_text("completed/a.json", &completed_doc("m1", 89, false))
            .await
            .unwrap();
        assert_eq!(sizing.observed_vram_gb(&store, "m1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn peak_above_smallest_live_gpu_is_dropped() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        // Fleet's smallest GPU is 80; a 96GB-box sample of the elastic
        // workload is not a valid lower bound and would fence the fleet off.
        publish(&store, "local-a", 80, &now_rfc3339()).await;
        store
            .upload_text("completed/a.json", &completed_doc("m1", 89, true))
            .await
            .unwrap();
        assert_eq!(sizing.observed_vram_gb(&store, "m1").await.unwrap(), None);
        // A second, fleet-representative sample governs via min().
        store
            .upload_text("completed/b.json", &completed_doc("m1", 74, true))
            .await
            .unwrap();
        // Fresh Sizing: the observed map is cached on the instance.
        let sizing = Sizing::new();
        assert_eq!(
            sizing.observed_vram_gb(&store, "m1").await.unwrap(),
            Some(74)
        );
    }

    #[tokio::test]
    async fn failed_oom_floor_raises_observed_but_not_past_fleet_ceiling() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        publish(&store, "local-a", 80, &now_rfc3339()).await;
        store
            .upload_text("completed/a.json", &completed_doc("m1", 50, true))
            .await
            .unwrap();
        // OOM floor 71 > observed 50 -> map entry raised to the floor.
        let failed = python_json_dumps(&json!({
            "job_id": "f1", "command": "x --model m1 --task t",
            "error": "CUDA out of memory. this process has 70.2 GiB memory in use",
        }))
        .unwrap();
        store.upload_text("failed/f1.json", &failed).await.unwrap();
        // Floor 90 > max live 80 -> ignored (cannot be served by the fleet).
        let failed2 = python_json_dumps(&json!({
            "job_id": "f2", "command": "x --model m2 --task t",
            "error": "this process has 89.5 GiB memory in use",
        }))
        .unwrap();
        store.upload_text("failed/f2.json", &failed2).await.unwrap();

        assert_eq!(
            sizing.observed_vram_gb(&store, "m1").await.unwrap(),
            Some(71)
        );
        assert_eq!(sizing.observed_vram_gb(&store, "m2").await.unwrap(), None);
    }

    #[tokio::test]
    async fn escalate_on_oom_requeues_to_next_real_gpu() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        publish(&store, "local-a", 24, &now_rfc3339()).await;
        publish(&store, "local-b", 80, &now_rfc3339()).await;

        let mut job = Job::new("j-oom", "run --model m-new --task t1");
        job.gpu_mem_gb = 24;
        job.state = job_state::RUNNING.into();
        job.started_at = Some(now_rfc3339());
        job.instance_ref = Some("agent@host-a".into());
        store.write_job("running", &job).await.unwrap();
        store
            .upload_text("status/j-oom/heartbeat", "x")
            .await
            .unwrap();

        let requeued = sizing
            .escalate_on_oom(
                &store,
                &mut job,
                "torch.cuda.OutOfMemoryError: CUDA out of memory",
            )
            .await
            .unwrap();
        assert!(requeued);
        assert_eq!(job.gpu_mem_gb, 80); // next REAL GPU up the ladder
        assert_eq!(job.state, "queued");
        assert!(job.failed_at.is_none() && job.error.is_none());
        assert!(job.instance_ref.is_none() && job.started_at.is_none());
        assert!(store.read_job("running", "j-oom").await.unwrap().is_none());
        assert_eq!(
            store
                .read_job("queue", "j-oom")
                .await
                .unwrap()
                .unwrap()
                .gpu_mem_gb,
            80
        );
        assert_eq!(store.read_status("j-oom").await.unwrap(), None);
    }

    #[tokio::test]
    async fn escalate_on_oom_uses_measured_floor_when_it_exceeds_current() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        publish(&store, "local-a", 96, &now_rfc3339()).await;
        let mut job = Job::new("j-floor", "run --model m-new --task t1");
        job.gpu_mem_gb = 24;
        store.write_job("running", &job).await.unwrap();
        // Floor 71 > current 24 -> jump straight to 71, not next_live_vram.
        let requeued = sizing
            .escalate_on_oom(
                &store,
                &mut job,
                "out of memory: this process has 70.2 GiB memory in use",
            )
            .await
            .unwrap();
        assert!(requeued);
        assert_eq!(job.gpu_mem_gb, 71);

        // Floor above the largest live GPU -> genuine failure, no requeue.
        let mut big = Job::new("j-big", "run --model m-new --task t1");
        big.gpu_mem_gb = 24;
        store.write_job("running", &big).await.unwrap();
        let requeued = sizing
            .escalate_on_oom(
                &store,
                &mut big,
                "out of memory: this process has 200 GiB memory in use",
            )
            .await
            .unwrap();
        assert!(!requeued);
        assert!(store.read_job("running", "j-big").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn escalate_refused_when_measured_peak_exists_or_no_bigger_gpu() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        publish(&store, "local-a", 80, &now_rfc3339()).await;
        store
            .upload_text("completed/a.json", &completed_doc("m-measured", 50, true))
            .await
            .unwrap();

        // Model already has a measured peak -> a real OOM is a real failure.
        let mut job = Job::new("j-m", "run --model m-measured --task t1");
        job.gpu_mem_gb = 80;
        store.write_job("running", &job).await.unwrap();
        assert!(!sizing
            .escalate_on_oom(&store, &mut job, "CUDA out of memory")
            .await
            .unwrap());
        assert!(store.read_job("running", "j-m").await.unwrap().is_some());

        // Unmeasured model on the largest live GPU -> genuine ceiling.
        let mut top = Job::new("j-top", "run --model m-other --task t1");
        top.gpu_mem_gb = 80;
        store.write_job("running", &top).await.unwrap();
        assert!(!sizing
            .escalate_on_oom(&store, &mut top, "CUDA out of memory")
            .await
            .unwrap());

        // Non-OOM error text -> never requeues.
        let mut other = Job::new("j-x", "run --model m-other --task t1");
        other.gpu_mem_gb = 24;
        assert!(!sizing
            .escalate_on_oom(&store, &mut other, "segfault")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn normalize_queue_sizing_forces_canonical_sizes() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        publish(&store, "local-a", 80, &now_rfc3339()).await;
        store
            .upload_text("completed/a.json", &completed_doc("m-measured", 50, true))
            .await
            .unwrap();

        // Unmeasured model with a stale hardcoded size -> forced to 0.
        let mut stale = Job::new("j-stale", "run --model m-new --task t1");
        stale.gpu_mem_gb = 64;
        store.write_job("queue", &stale).await.unwrap();
        // Measured model with a stale size -> stamped with the peak.
        let mut measured = Job::new("j-meas", "run --model m-measured --task t1");
        measured.gpu_mem_gb = 80;
        store.write_job("queue", &measured).await.unwrap();
        // Already-canonical entries are left alone.
        let mut ok = Job::new("j-ok", "run --model m-measured --task t2");
        ok.gpu_mem_gb = 50;
        store.write_job("queue", &ok).await.unwrap();

        let logs: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let corrected = sizing
            .normalize_queue_sizing(&store, &|m| logs.lock().unwrap().push(m.to_string()))
            .await
            .unwrap();
        assert_eq!(corrected, 2);
        assert_eq!(
            store
                .read_job("queue", "j-stale")
                .await
                .unwrap()
                .unwrap()
                .gpu_mem_gb,
            0
        );
        assert_eq!(
            store
                .read_job("queue", "j-meas")
                .await
                .unwrap()
                .unwrap()
                .gpu_mem_gb,
            50
        );
        assert!(logs.lock().unwrap()[0].contains("normalized 2 queue jobs"));

        // Second pass is a no-op.
        let corrected = sizing
            .normalize_queue_sizing(&store, &|_| ())
            .await
            .unwrap();
        assert_eq!(corrected, 0);
    }

    #[tokio::test]
    async fn observed_map_ignores_malformed_state_and_non_json_siblings() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        publish(&store, "local-a", 80, &now_rfc3339()).await;
        // state != completed -> not a successful observation.
        let wrong_state = python_json_dumps(&json!({
            "job_id": "w", "command": "x --model m1 --task t", "state": "failed",
            "peak_vram_gb": 12, "peak_vram_per_gpu": true,
        }))
        .unwrap();
        store
            .upload_text("completed/w.json", &wrong_state)
            .await
            .unwrap();
        assert_eq!(sizing.observed_vram_gb(&store, "m1").await.unwrap(), None);
    }
}
