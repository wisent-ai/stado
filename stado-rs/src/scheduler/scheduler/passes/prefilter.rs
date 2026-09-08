//! Pass one: the metadata-only candidate window.

use std::collections::{BTreeMap, BTreeSet};

use crate::config;

/// The metadata-only prefilter + priority-desc/FIFO ordering half of
/// Python `schedule_queued_jobs`, split out for tests. Returns the ordered
/// candidate job ids (already capped to `window_budget`) and the count of
/// jobs skipped for sitting on a 0-quota accelerator.
///
/// Metadata-only prefilter (NO body downloads): keep only jobs whose
/// accelerator has available quota this tick, so a backlog of
/// UNDISPATCHABLE jobs cannot saturate the per-tick window and starve
/// dispatchable work. Confirmed live 2026-06-01: 435 jobs sized to
/// nvidia-tesla-k80 (0 fleet k80 quota) filled the 200-job FIFO window
/// every tick -> the only bucket formed was k80 -> "Skip:
/// 0 quota" ->
/// scheduled 0 for the WHOLE fleet, including brand-new t4/l4 jobs queued
/// behind the stuck backlog. write_job stamps gpu_mem_gb, gpu_type, and
/// priority into blob metadata, so this filters + orders the whole queue
/// cheaply and we read only the surviving window's bodies.
/// The stuck backlog stays queued and untouched — it just stops blocking.
pub(super) fn prefilter_candidates_with_routing(
    blobs: &[crate::queue::BlobInfo],
    available: &BTreeMap<String, i64>,
    provider_name: &str,
    window_budget: usize,
    require_provider_pin: bool,
) -> (Vec<String>, usize) {
    let in_quota: BTreeSet<&str> = available
        .iter()
        .filter(|(_, available)| **available > i64::default())
        .map(|(accelerator, _)| accelerator.as_str())
        .collect();
    let mut cand: Vec<(i64, i64, String)> = Vec::new();
    let mut skipped_no_quota = usize::default();
    for info in blobs {
        if !info.name.ends_with(".json") {
            continue;
        }
        let meta = &info.metadata;
        if require_provider_pin
            && (meta.get("pin_to_provider").map(String::as_str) != Some("true")
                || meta.get("provider").map(String::as_str) != Some(provider_name))
        {
            continue;
        }
        let gm: i64 = meta
            .get("gpu_mem_gb")
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let explicit_accel = meta.get("gpu_type").map(|value| value.trim()).unwrap_or("");
        let derived = if gm > i64::default() {
            let (_, accelerator) = config::lookup_instance_type(provider_name, gm);
            accelerator
        } else {
            ""
        };
        let accel_for_filter = if explicit_accel.is_empty() {
            derived
        } else {
            explicit_accel
        };
        if !accel_for_filter.is_empty() && !in_quota.contains(accel_for_filter) {
            skipped_no_quota += true as usize;
            continue;
        }
        let prio: i64 = meta
            .get("priority")
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let ts = info
            .updated
            .map(|updated| updated.timestamp())
            .unwrap_or_default();
        let jid = info
            .name
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".json")
            .to_string();
        cand.push((-prio, ts, jid));
    }
    cand.sort_by_key(|left| (left.0, left.1));
    cand.truncate(window_budget);
    (
        cand.into_iter().map(|(_, _, job_id)| job_id).collect(),
        skipped_no_quota,
    )
}
