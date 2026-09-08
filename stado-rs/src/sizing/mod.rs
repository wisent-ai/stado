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
use std::time::Instant;

use tokio::sync::Mutex;

use crate::constants;
use crate::queue::{JobStorage, StorageError};

mod escalate;
mod fleet;
mod observed;
mod parse;

pub use parse::{is_oom_error, model_of, oom_required_gb};

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
