//! Python-compatible formatting helpers: `datetime.isoformat()`,
//! `ensure_ascii=True`, `sort_keys=True` and `repr()` of a string or a list
//! of strings. These are the only copies in Stado: every refusal and log line
//! that quotes a value the Python way renders it here.

use serde_json::Value;

/// Python `datetime.isoformat()` for a UTC datetime: `+00:00` suffix, with
/// 6-digit microseconds only when nonzero (Python omits the fraction when
/// `microsecond == 0`).
pub(crate) fn isoformat_utc(dt: chrono::DateTime<chrono::Utc>) -> String {
    if std::num::NonZeroU32::new(dt.timestamp_subsec_micros()).is_none() {
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
        // Python escapes everything outside printable ASCII. The pretty
        // printer's own line breaks and indentation stay as they are, and
        // serde_json has already escaped the controls inside strings, which
        // leaves DEL.
        if ch.is_ascii() && (ch.is_ascii_whitespace() || !ch.is_ascii_control()) {
            out.push(ch);
        } else {
            for unit in String::from(ch).encode_utf16() {
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
/// for the quote, backslash, and the ASCII control characters.
pub(crate) fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len());
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
            c if c.is_ascii_control() => {
                out.push_str(&format!("\\x{:02x}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr()` of a list of strings: `['a', 'b']`, each item quoted and
/// escaped by [`py_str_repr`], so an item holding a quote reads back as itself.
pub(crate) fn py_list_repr<S: AsRef<str>>(items: &[S]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| py_str_repr(item.as_ref())).collect();
    format!("[{}]", quoted.join(", "))
}
