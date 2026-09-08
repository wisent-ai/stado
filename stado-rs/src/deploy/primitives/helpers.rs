//! Python-compatible quoting / repr helpers, plus the idempotent file write
//! the installers share.

// ---------------------------------------------------------------------------
// Python-compatible quoting / repr helpers
// ---------------------------------------------------------------------------

/// Python `shlex.quote`: safe chars (`[a-zA-Z0-9_@%+=:,./-]`) pass through,
/// anything else is single-quoted with `'` escaped as `'"'"'`.
pub fn shlex_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let safe = |b: u8| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    };
    if value.bytes().all(safe) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Python `repr()` of a (simple) string: single quotes by default, double
/// quotes when the value contains a single quote but no double quote;
/// backslash, the quote char, and `\n`/`\r`/`\t` are escaped.
pub fn py_str_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr()` of a list of strings: `['a', 'b']`.
pub fn py_list_repr(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| py_str_repr(item)).collect();
    format!("[{}]", quoted.join(", "))
}

/// Python `str()` of a `dict[str, str]` (`{'K': 'V', ...}`), preserving the
/// given insertion order. Used by the `bootstrap --local --dry-run` env line.
pub fn py_dict_repr(items: &[(String, String)]) -> String {
    let pairs: Vec<String> = items
        .iter()
        .map(|(key, value)| format!("{}: {}", py_str_repr(key), py_str_repr(value)))
        .collect();
    format!("{{{}}}", pairs.join(", "))
}

/// Write `content` to `path` only when it differs from what is already on
/// disk; returns true when the file was (re)written. The Python installers
/// rewrite unconditionally and declare idempotency at the "same resulting
/// state" level; skipping the byte-identical rewrite keeps mtime (and any
/// watching daemon) stable without changing the resulting state.
pub fn write_if_changed(path: &std::path::Path, content: &str) -> Result<bool, std::io::Error> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(false);
        }
    }
    std::fs::write(path, content)?;
    Ok(true)
}
