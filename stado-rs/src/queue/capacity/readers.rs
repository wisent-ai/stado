//! Aggregate reads over the capacity broadcasts, split out of
//! [`super`] so both files stay small enough to change.

use super::*;

/// Whether a publication is of one of the asked kinds; a publication that
/// states no kind is of none of them.
fn of_asked_kind(payload: &Value, kinds: &[&str]) -> bool {
    payload
        .get("kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| kinds.contains(&kind))
}

/// Sum available accelerator placements across accepting workers, optionally
/// filtered by worker kind.
pub fn total_available_accelerators(
    consumers: &BTreeMap<String, Value>,
    kinds: Option<&[&str]>,
) -> BTreeMap<String, i64> {
    let mut totals: BTreeMap<String, i64> = BTreeMap::new();
    for payload in consumers.values() {
        if payload.get("accepting_jobs").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        if kinds.is_some_and(|kinds| !of_asked_kind(payload, kinds)) {
            continue;
        }
        let Some(available) = payload
            .get("available_accelerators")
            .and_then(Value::as_object)
        else {
            continue;
        };
        // A count that is not a number is not a count: the accelerator is
        // not totalled, rather than totalled as none.
        for (accelerator, count) in available {
            let Some(count) = count.as_i64() else {
                continue;
            };
            let total = totals.entry(accelerator.clone()).or_default();
            *total = total.saturating_add(count);
        }
    }
    totals
}

/// [(consumer_id, claimable_vram_gb), ...] sorted descending: each agent's
/// published free VRAM, which is exactly what its claim rule admits a job
/// against.
pub fn consumers_by_claimable_vram(
    consumers: &BTreeMap<String, Value>,
    kinds: Option<&[&str]>,
) -> Vec<(String, i64)> {
    let mut rows: Vec<(String, i64)> = Vec::new();
    for payload in consumers.values() {
        if kinds.is_some_and(|kinds| !of_asked_kind(payload, kinds)) {
            continue;
        }
        let number = |value: &Value| value.as_i64().or_else(|| value.as_f64().map(|f| f as i64));
        let Some(free) = payload.get("free_vram_gb").and_then(number) else {
            continue;
        };
        // Python `payload["consumer_id"]`; read_consumer_capacity only emits
        // payloads that carry the key.
        let Some(cid) = payload.get("consumer_id").and_then(Value::as_str) else {
            continue;
        };
        rows.push((cid.to_string(), free));
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.1));
    rows
}
