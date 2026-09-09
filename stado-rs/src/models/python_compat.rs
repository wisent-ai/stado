//! Python-compatible formatting helpers: `datetime.isoformat()`,
//! `ensure_ascii=True`, `sort_keys=True` and `repr()` of a string.

use serde_json::Value;

/// Python `datetime.isoformat()` for a UTC datetime: `+00:00` suffix, with
/// 6-digit microseconds only when nonzero (Python omits the fraction when
/// `microsecond == 0`).
pub(crate) fn isoformat_utc(dt: chrono::DateTime<chrono::Utc>) -> String {
    if dt.timestamp_subsec_micros() == 0 {
        dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// Replicates Python's `ensure_ascii=True`: escapes every non-ASCII char as
/// \uXXXX (with surrogate pairs for astral planes). Already-escaped sequences
/// and structural characters are untouched.
pub(crate) fn ensure_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if (ch as u32) < 0x7f {
            out.push(ch);
        } else {
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{:04x}", unit));
            }
        }
    }
    out
}

/// Recursively sort object keys (Python `sort_keys=True`).
pub(crate) fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let btree: std::collections::BTreeMap<String, Value> =
                map.iter().map(|(k, v)| (k.clone(), sort_keys(v))).collect();
            Value::Object(btree.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// Python `json.dumps(value, indent=2, sort_keys=True)` (ensure_ascii=True).
pub(crate) fn json_dumps_pretty_sorted(value: &Value) -> String {
    let pretty =
        serde_json::to_string_pretty(&sort_keys(value)).expect("JSON serialization is infallible");
    ensure_ascii(&pretty)
}

/// Python `repr()` of a string: single quotes by default, double quotes when
/// the string contains a single quote (and no double quote); backslash-escapes
/// for the quote, backslash, and the usual control characters.
pub(crate) fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
