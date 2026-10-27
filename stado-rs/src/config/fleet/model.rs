//! Model policy, compute API origin and instance sizing.

use std::sync::{LazyLock, RwLock};

use crate::catalog::GPU_SIZING;

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
}

static MODEL_POLICY: LazyLock<RwLock<ModelPolicyCache>> =
    LazyLock::new(|| RwLock::new(ModelPolicyCache::default()));

/// Read the shared policy from the store; the coordinator and agent ticks
/// call this once per pass, so a change is in force from the next pass. A
/// missing blob means an intentionally empty policy; transport or JSON
/// errors leave the last good value untouched and are returned to the caller
/// for logging.
pub async fn refresh_model_policy(
    store: &crate::queue::JobStorage,
) -> Result<ModelPolicy, crate::queue::StorageError> {
    let policy = match store.download_text("config/model_overrides.json").await? {
        Some(raw) => serde_json::from_str::<ModelPolicy>(&raw)?,
        None => ModelPolicy::default(),
    };
    let mut cache = match MODEL_POLICY.write() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    cache.policy = policy.clone();
    Ok(policy)
}

/// Last successfully fetched policy.
pub fn model_policy() -> ModelPolicy {
    match MODEL_POLICY.read() {
        Ok(cache) => cache.policy.clone(),
        Err(poisoned) => poisoned.into_inner().policy.clone(),
    }
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

/// Estimate GPU memory needed from a command string: the model it names
/// ([`crate::sizing::model_of`]), sized by the smallest measured peak the
/// fleet recorded for that model, else by the smallest GPU a live agent
/// broadcasts; `None` when the command names no model, or nothing is
/// measured and no live GPU is broadcast.
///
/// Sizing is purely the measured peak. No params formula, no per-model
/// constant, no multiplier, no hand-written tier ladder. With no
/// measurement the job starts on the smallest GPU that actually exists in
/// the fleet now; if it runs out of memory there,
/// `sizing.escalate_on_oom` moves it to the next larger real GPU, and that
/// run's measured peak sizes every later job of the model. A storage outage
/// propagates, which the submit path surfaces as a submit error.
pub async fn estimate_gpu_memory(
    command: &str,
    sizing: &crate::sizing::Sizing,
    store: &crate::queue::JobStorage,
) -> Result<Option<i64>, crate::queue::StorageError> {
    let Some(model) = crate::sizing::model_of(command) else {
        return Ok(None);
    };
    if let Some(measured) = sizing.observed_vram_gb(store, &model).await? {
        return Ok(Some(measured));
    }
    sizing.smallest_live_vram(store).await
}

/// The (machine_type, accel_type) of the smallest tier of `provider`'s GPU
/// sizing that holds `gpu_mem_gb`, or of its largest tier when none does:
/// the in-VM workload may still run out of memory there, a clearer failure
/// than a malformed instance request. `None` when the provider has no GPU
/// sizing at all.
pub fn lookup_instance_type(
    provider: &str,
    gpu_mem_gb: i64,
) -> Option<(&'static str, &'static str)> {
    let sizing = GPU_SIZING.get(provider)?;
    sizing
        .range(gpu_mem_gb..)
        .next()
        .or_else(|| sizing.iter().next_back())
        .map(|(_, spec)| *spec)
}
