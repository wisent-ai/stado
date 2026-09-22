//! What the fleet's own agents published about themselves, read for the host
//! a session is about to be placed on.
//!
//! Split out of `jeden/mod.rs`, which had grown past the module line cap; the
//! candidates and the readiness probe stay there.

use serde_json::Value;

use crate::deploy::host_channel;
use crate::targets::ComputeTarget;

pub(super) async fn live_capacity() -> Vec<Value> {
    let Ok(store) = crate::queue::submit::default_store("").await else {
        return Vec::new();
    };
    crate::queue::capacity::read_consumer_capacity(&store)
        .await
        .map(|entries| entries.into_values().collect())
        .unwrap_or_default()
}

/// The publication this target's own agent wrote, when it wrote one.
fn live_entry<'a>(target: &ComputeTarget, capacity: &'a [Value]) -> Option<&'a Value> {
    let hostnames = target
        .hostnames
        .iter()
        .map(|host| crate::targets::normalize_hostname(host))
        .collect::<Vec<_>>();
    capacity
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("local"))
        .find(|entry| {
            let consumer = entry
                .get("consumer_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            hostnames.iter().any(|host| {
                consumer == format!("local-{host}")
                    || consumer
                        .strip_prefix("local-")
                        .is_some_and(|value| crate::targets::normalize_hostname(value) == *host)
            })
        })
}

/// Why `target` cannot take work right now, in its own agent's words, or
/// nothing when it is accepting.
///
/// A detached session is pinned to the host its placement chose, so a host
/// that publishes `accepting_jobs: false` would hold the session queued
/// until that clears. Both Macs reported `disk_pressure_active` on
/// 2026-09-19 while a session sat pinned to one of them; refusing here
/// moves the placement to a host that can claim it.
pub(super) fn admission_refusal(target: &ComputeTarget, capacity: &[Value]) -> Option<String> {
    let live = live_entry(target, capacity)?;
    if live
        .get("accepting_jobs")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let reason = live
        .get("diag")
        .and_then(|diag| diag.get("admission_reason"))
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
        .unwrap_or("its agent published no reason");
    Some(format!(
        "{} is not accepting placements ({reason})",
        target.name
    ))
}

pub(super) fn target_score(target: &ComputeTarget, capacity: &[Value]) -> i64 {
    let live = live_entry(target, capacity);
    let accepting = live
        .and_then(|entry| entry.get("accepting_jobs"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let available_cpu_cores = live
        .and_then(|entry| entry.get("available_cpu_cores"))
        .and_then(Value::as_i64)
        .unwrap_or_default()
        .max(0);
    let live_bonus = if accepting { 1_000_000 } else { 0 };
    let local_bonus = i64::from(host_channel::target_is_this_host(target));
    live_bonus + available_cpu_cores.saturating_mul(1_000) + local_bonus
}
