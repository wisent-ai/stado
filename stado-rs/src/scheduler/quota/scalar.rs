//! The Python `int()` coercion every quota dict read goes through, shared
//! by the live per-provider readers and the reservation overlay.

use serde_json::Value;

/// Python `int(value)` for JSON scalars in the quota dicts, with Python's
/// default of 0 for missing keys. Deviation: Python's `int()` raises
/// ValueError on a non-numeric string; this port treats garbage as 0 (the
/// live API only ever emits numbers, and the overlay is operator-written).
pub(in crate::scheduler::quota) fn py_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}
