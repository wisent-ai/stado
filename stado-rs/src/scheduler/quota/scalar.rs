//! The Python `int()` coercion every quota dict read goes through, shared
//! by the live per-provider readers and the reservation overlay.

use serde_json::Value;

use super::QuotaError;

/// Python `int(value)` for JSON scalars in the quota dicts: `None` for an
/// absent key (the callers count it as nothing, Python's default), the number
/// for a number or a numeric string (a float truncates), and a refusal naming
/// the field and the value for anything else. Garbage in the operator-written
/// overlay used to read as 0, so a misspelt reservation freed the capacity it
/// was meant to hold.
pub(in crate::scheduler::quota) fn py_int(
    field: &str,
    value: Option<&Value>,
) -> Result<Option<i64>, QuotaError> {
    let refused = |value: &Value| QuotaError::NotACount {
        field: field.to_string(),
        value: value.to_string(),
    };
    match value {
        None => Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .map(Some)
            .ok_or_else(|| refused(&Value::Number(n.clone()))),
        Some(Value::String(s)) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| refused(&Value::String(s.clone()))),
        Some(other) => Err(refused(other)),
    }
}
