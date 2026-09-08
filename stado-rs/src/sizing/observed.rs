//! The cached model -> measured-peak map and the lookup that reads it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::queue::{JobStorage, StorageError};

use super::{
    download_many, model_of, oom_required_gb, Sizing, COMPLETED_SAMPLE_CAP, OBSERVED_MAP_TTL_S,
};

impl Sizing {
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
}
