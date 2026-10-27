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
/// dispatchable work. A backlog of jobs sized to an accelerator with zero
/// fleet quota fills the FIFO window every tick, the only bucket formed is
/// that one, it skips on "0 quota", and the WHOLE fleet schedules nothing,
/// including brand-new jobs queued behind the stuck backlog.
/// write_job stamps gpu_mem_gb, gpu_type, and
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
        .filter(|(_, available)| available.is_positive())
        .map(|(accelerator, _)| accelerator.as_str())
        .collect();
    // Ordered by priority, highest first (a job that states none after every
    // job that does), then by when it was written (one with no time first).
    let mut cand: Vec<(std::cmp::Reverse<Option<i64>>, Option<i64>, String)> = Vec::new();
    let mut skipped_no_quota: Vec<&str> = Vec::new();
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
        // The metadata stamps are a copy of the job body, read here only to
        // order and filter without downloading bodies. A stamp that is absent
        // or does not parse states nothing: the job is kept in the window and
        // its body, read next, decides.
        let stamped = |key: &str| -> Option<i64> {
            meta.get(key).and_then(|value| match value.parse::<i64>() {
                Ok(number) => Some(number),
                Err(_unparsed) => None,
            })
        };
        let explicit_accel = match meta.get("gpu_type") {
            Some(value) => value.trim(),
            None => "",
        };
        let derived = match stamped("gpu_mem_gb") {
            Some(gm) if gm.is_positive() => match config::lookup_instance_type(provider_name, gm) {
                Some((_, accel)) => accel,
                None => "",
            },
            Some(_) | None => "",
        };
        let accel_for_filter = if explicit_accel.is_empty() {
            derived
        } else {
            explicit_accel
        };
        if !accel_for_filter.is_empty() && !in_quota.contains(accel_for_filter) {
            skipped_no_quota.push(&info.name);
            continue;
        }
        let Some(file) = info.name.rsplit('/').next() else {
            continue;
        };
        let jid = file.trim_end_matches(".json").to_string();
        cand.push((
            std::cmp::Reverse(stamped("priority")),
            info.updated.map(|updated| updated.timestamp()),
            jid,
        ));
    }
    cand.sort_by_key(|left| (left.0, left.1));
    cand.truncate(window_budget);
    (
        cand.into_iter().map(|(_, _, job_id)| job_id).collect(),
        skipped_no_quota.len(),
    )
}
