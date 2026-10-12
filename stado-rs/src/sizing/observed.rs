//! The cached model -> measured-peak map and the lookup that reads it.

use std::collections::HashMap;

use serde_json::Value;

use crate::queue::{JobStorage, StorageError};

use super::{download_many, model_of, oom_required_gb, ObservedInputs, Sizing};

/// The record directories the map is built from.
const COMPLETED_PREFIX: &str = "completed/";
const FAILED_PREFIX: &str = "failed/";
/// The state a completed record carries.
const COMPLETED_STATE: &str = "completed";

/// Every record name under `prefix`, in name order, so two listings of the
/// same records compare equal.
async fn record_names(store: &JobStorage, prefix: &str) -> Result<Vec<String>, StorageError> {
    let mut names: Vec<String> = store
        .list_blobs_with_meta(prefix)
        .await?
        .into_iter()
        .map(|blob| blob.name)
        .collect();
    names.sort();
    Ok(names)
}

/// The model a record's command runs, when it names one.
fn record_model(doc: &Value) -> Option<String> {
    let model = model_of(doc.get("command").and_then(Value::as_str)?);
    (!model.is_empty()).then_some(model)
}

impl Sizing {
    /// Smallest demonstrated-sufficient MEASURED peak_vram_gb for `model`
    /// (min over its successful per-GPU-probe completions), or None if the
    /// model has no such measured completion yet (caller must NOT fabricate
    /// a number — start on the smallest ACTUAL fleet GPU and escalate via
    /// live capacities). Python `observed_vram_gb`.
    ///
    /// The map is rebuilt exactly when what it is built from changed: the
    /// completed and failed job records the queue lists, and the GPUs the
    /// live fleet publishes. The listings are read on every call (names
    /// only); the records are downloaded again only when a listing or the
    /// live GPUs differ from the ones the held map was built from. No time
    /// decides it.
    pub async fn observed_vram_gb(
        &self,
        store: &JobStorage,
        model: &str,
    ) -> Result<Option<i64>, StorageError> {
        Ok(self.observed_map(store).await?.get(model).copied())
    }

    /// The whole model -> measured-peak map, current as of this call. A
    /// caller that looks up many models in one pass reads it once and looks
    /// them up in the returned map, so one pass costs one set of listings.
    pub async fn observed_map(
        &self,
        store: &JobStorage,
    ) -> Result<HashMap<String, i64>, StorageError> {
        let inputs = ObservedInputs {
            completed: record_names(store, COMPLETED_PREFIX).await?,
            failed: record_names(store, FAILED_PREFIX).await?,
            live_vrams: self.live_total_vrams(store).await?,
        };
        let mut cache = self.observed.lock().await;
        let current = match (&cache.inputs, &cache.map) {
            (Some(held), Some(map)) if *held == inputs => Some(map.clone()),
            _ => None,
        };
        if let Some(map) = current {
            return Ok(map);
        }
        // A hard failure propagates; the cache keeps the last good map
        // until a later rebuild succeeds (see build_observed_map).
        let map = self.build_observed_map(store, &inputs).await?;
        cache.map = Some(map.clone());
        cache.inputs = Some(inputs);
        Ok(map)
    }

    /// model -> min measured peak_vram_gb over its completed runs.
    /// Python `_build_observed_map`.
    ///
    /// Any model with a real measurement is included; there is no
    /// minimum-sample gate. A coordinator-side storage list/read outage
    /// must not silently erase the map fleet-wide, so a hard failure here
    /// propagates; the caller's cache keeps the last good map until a
    /// later rebuild succeeds.
    async fn build_observed_map(
        &self,
        store: &JobStorage,
        inputs: &ObservedInputs,
    ) -> Result<HashMap<String, i64>, StorageError> {
        // Every completed record is a measurement; no sample count is chosen.
        let mut peaks: HashMap<String, Vec<i64>> = HashMap::new();
        for text in download_many(store, &inputs.completed)
            .await?
            .into_iter()
            .flatten()
        {
            let doc: Value = serde_json::from_str(&text)?;
            if doc.get("state").and_then(Value::as_str) != Some(COMPLETED_STATE) {
                continue;
            }
            // Python `isinstance(peak, int)`: a JSON float (74.0) is not an
            // int and as_i64 rejects it the same way. A peak that is not
            // positive is an unmeasured or CPU job, not an observation.
            let Some(peak) = doc
                .get("peak_vram_gb")
                .and_then(Value::as_u64)
                .and_then(std::num::NonZeroU64::new)
                .and_then(|peak| i64::try_from(peak.get()).ok())
            else {
                continue;
            };
            if doc.get("peak_vram_per_gpu") != Some(&Value::Bool(true)) {
                // Legacy record from the probe that summed used_memory ACROSS
                // GPUs (cross-GPU total, not per-card). Mixing those into the
                // per-model sample set corrupts the signal, so only peaks the
                // corrected per-GPU probe produced are trusted. Until a model
                // has such a record observed_vram_gb returns None and the job
                // sizes via the smallest-live-GPU + OOM-escalate path (no
                // fabricated number).
                continue;
            }
            let Some(model) = record_model(&doc) else {
                continue;
            };
            peaks.entry(model).or_default().push(peak);
        }

        // Memory-elastic workloads can fill a larger card without requiring
        // that capacity. Exclude peaks above the smallest live GPU so they
        // do not fence the workload off smaller hosts. With no usable peak,
        // normal first-run sizing and escalation produce a new measurement.
        let smallest_live = inputs.live_vrams.first().copied();
        let mut out: HashMap<String, i64> = HashMap::new();
        for (model, samples) in peaks {
            let usable = samples
                .into_iter()
                .filter(|p| smallest_live.is_none_or(|sl| *p <= sl))
                .min();
            if let Some(min) = usable {
                out.insert(model, min);
            }
        }

        // A failed record whose PyTorch message proves a floor raises the
        // model's size to it, unless no live GPU could hold that floor.
        let max_live_vram = inputs.live_vrams.last().copied();
        let mut floors: HashMap<String, i64> = HashMap::new();
        for text in download_many(store, &inputs.failed)
            .await?
            .into_iter()
            .flatten()
        {
            let doc: Value = serde_json::from_str(&text)?;
            let Some(model) = record_model(&doc) else {
                continue;
            };
            let Some(floor) = doc
                .get("error")
                .and_then(Value::as_str)
                .and_then(oom_required_gb)
            else {
                continue;
            };
            if max_live_vram.is_some_and(|mx| floor > mx) {
                continue;
            }
            if floors.get(&model).is_none_or(|held| floor > *held) {
                floors.insert(model, floor);
            }
        }
        for (model, floor) in floors {
            let raised = match out.get(&model) {
                Some(peak) => (*peak).max(floor),
                None => floor,
            };
            out.insert(model, raised);
        }
        Ok(out)
    }
}
