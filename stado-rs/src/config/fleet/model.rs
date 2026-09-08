//! Model policy, compute API origin and instance sizing.

use std::sync::{LazyLock, RwLock};
use std::time::Instant;

use crate::catalog::GPU_SIZING;

/// In-process cache TTL for the model policy loaded through the configured
/// [`crate::queue::JobStorage`] adapter.
pub const MODEL_POLICY_TTL_S: u64 = 300;

/// Co-schedule and cost-policy flags loaded from the provider-neutral
/// `config/model_overrides.json` object in the configured queue store.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct ModelPolicy {
    pub exclusive: Vec<String>,
    pub local_only: Vec<String>,
}

#[derive(Debug, Default)]
struct ModelPolicyCache {
    policy: ModelPolicy,
    fetched_at: Option<Instant>,
}

static MODEL_POLICY: LazyLock<RwLock<ModelPolicyCache>> =
    LazyLock::new(|| RwLock::new(ModelPolicyCache::default()));

/// Refresh the shared policy when its TTL expires. A missing blob means an
/// intentionally empty policy; transport or JSON errors leave the last good
/// value untouched and are returned to the caller for logging.
pub async fn refresh_model_policy(
    store: &crate::queue::JobStorage,
) -> Result<ModelPolicy, crate::queue::StorageError> {
    {
        let cache = MODEL_POLICY
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache
            .fetched_at
            .is_some_and(|at| at.elapsed().as_secs() < MODEL_POLICY_TTL_S)
        {
            return Ok(cache.policy.clone());
        }
    }

    let policy = match store.download_text("config/model_overrides.json").await? {
        Some(raw) => serde_json::from_str::<ModelPolicy>(&raw)?,
        None => ModelPolicy::default(),
    };
    let mut cache = MODEL_POLICY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.policy = policy.clone();
    cache.fetched_at = Some(Instant::now());
    Ok(policy)
}

/// Last successfully fetched policy.
pub fn model_policy() -> ModelPolicy {
    MODEL_POLICY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .policy
        .clone()
}

/// True when `model` must run with exclusive GPU ownership.
pub fn is_exclusive_model(model: &str) -> bool {
    model_policy()
        .exclusive
        .iter()
        .any(|candidate| candidate == model)
}

/// True when `model` is restricted to local execution.
pub fn is_local_only_model(model: &str) -> bool {
    model_policy()
        .local_only
        .iter()
        .any(|candidate| candidate == model)
}

/// Compute API base URL (env `COMPUTE_API_URL`). Python resolves this at
/// import time in `stado/queue/submit.py` (`COMPUTE_API`).
static COMPUTE_API: LazyLock<String> = LazyLock::new(|| {
    std::env::var("COMPUTE_API_URL").unwrap_or_else(|_| "https://compute.wisent.com".to_string())
});

/// Base URL of the compute.wisent.com API (env `COMPUTE_API_URL`).
pub fn compute_api() -> &'static str {
    COMPUTE_API.as_str()
}

/// Estimate GPU memory needed from a command string.
///
/// Port of `stado/config.py::estimate_gpu_memory`. The model-name regex
/// extraction (`--model\s+(\S+)`, quote-stripped) is byte-faithful.
/// Python's call is sync over a GCS scan (`sizing.observed_vram_gb` /
/// `sizing.smallest_live_vram`); here the scan is async and goes through
/// the passed [`JobStorage`] + [`crate::sizing::Sizing`] cache holder.
/// A storage outage propagates (Python: the sizing rebuild raises), which
/// the submit path surfaces as a submit error.
pub async fn estimate_gpu_memory(
    command: &str,
    sizing: &crate::sizing::Sizing,
    store: &crate::queue::JobStorage,
) -> Result<i64, crate::queue::StorageError> {
    static MODEL_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"--model\s+(\S+)").expect("static regex compiles"));
    let Some(caps) = MODEL_RE.captures(command) else {
        return Ok(0);
    };
    let model = caps[1].trim_matches(['\'', '"']);

    // Sizing is PURELY the measured peak. No params formula, no per-model
    // constant, no multiplier, no hand-written tier ladder — all forbidden
    // hardcoded guesses. observed_vram_gb returns the min real nvidia-smi
    // peak_vram_gb the fleet has recorded for this model.
    if let Some(measured) = sizing.observed_vram_gb(store, model).await? {
        return Ok(measured);
    }

    // No measurement yet: do NOT fabricate a number. Start on the
    // smallest GPU that ACTUALLY EXISTS in the fleet right now (read from
    // live capacity broadcasts, not a catalog). If it OOMs there,
    // slots.advance_slot -> sizing.escalate_on_oom moves it to the next
    // larger REAL fleet GPU, repeating until it runs; that run's measured
    // nvidia-smi peak then sizes every later job of this model. If no
    // live agent is broadcasting, return 0 (unsized) rather than invent a
    // size — the job waits for a real GPU to appear.
    Ok(sizing.smallest_live_vram(store).await?.unwrap_or(0))
}

/// Return (machine_type, accel_type) for the given memory requirement.
///
/// If gpu_mem_gb exceeds every tier in GPU_SIZING, returns the LARGEST
/// available tier rather than ("", ""). The previous behavior produced an
/// empty machine_type that the GCE create_instance call rejected with
/// 'Machine type with name "" does not exist', wedging the job. Sending it
/// to the largest tier means the in-VM workload may still OOM, but that's a
/// clearer failure mode than a malformed instance request.
pub fn lookup_instance_type(provider: &str, gpu_mem_gb: i64) -> (&'static str, &'static str) {
    let Some(sizing) = GPU_SIZING.get(provider) else {
        return ("", "");
    };
    if let Some((_, spec)) = sizing.range(gpu_mem_gb..).next() {
        return *spec;
    }
    sizing
        .iter()
        .next_back()
        .map(|(_, spec)| *spec)
        .unwrap_or(("", ""))
}
