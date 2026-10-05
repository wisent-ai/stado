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
//! estimate call would blow the tick budget, so the map is held in process
//! and rebuilt only when the completed/ or failed/ listing, or the live
//! fleet's GPUs, differ from what it was built from.
//!
//! Python keeps the caches as module globals; here they live on the
//! [`Sizing`] struct (async storage access needs an owner) and the
//! process-wide instance behind [`global()`] reproduces the module-global
//! semantics for CLI/coordinator one-shot callers. Tests construct their
//! own `Sizing::new()` so caches never leak between cases.

use std::collections::HashMap;
use std::sync::LazyLock;

use tokio::sync::Mutex;

use crate::queue::{JobStorage, StorageError};

mod escalate;
mod fleet;
mod observed;
mod parse;

pub use parse::{is_oom_error, model_of, oom_required_gb};

/// In-process cache for the observed-VRAM map. Cheap to construct; clone via
/// [`global()`] for the process-wide instance.
pub struct Sizing {
    observed: Mutex<ObservedCache>,
}

/// What one observed map was built from: the completed and failed records
/// the queue listed and the GPUs the live fleet published.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedInputs {
    completed: Vec<String>,
    failed: Vec<String>,
    live_vrams: Vec<i64>,
}

#[derive(Default)]
struct ObservedCache {
    map: Option<HashMap<String, i64>>,
    inputs: Option<ObservedInputs>,
}

impl Default for Sizing {
    fn default() -> Self {
        Self::new()
    }
}

impl Sizing {
    pub fn new() -> Self {
        Self {
            observed: Mutex::new(ObservedCache::default()),
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

/// Parallel-download the given blob paths, as wide as the machine is, path
/// order preserved. A missing blob (TOCTOU: moved between list and download)
/// comes back as None and is skipped by the caller; any other error
/// propagates so a real outage is visible.
async fn download_many(
    store: &JobStorage,
    paths: &[String],
) -> Result<Vec<Option<String>>, StorageError> {
    use futures::StreamExt;
    futures::stream::iter(paths)
        .map(|path| store.download_text(path))
        .buffered(crate::queue::migrations::bulk_workers())
        .collect::<Vec<Result<Option<String>, StorageError>>>()
        .await
        .into_iter()
        .collect()
}
