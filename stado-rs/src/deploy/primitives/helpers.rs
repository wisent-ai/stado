//! Python-compatible shell quoting, plus the idempotent file write the
//! installers share. Python `repr()` lives in `models::python_compat`.

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

/// Write `content` to `path` only when it differs from what is already on
/// disk; returns true when the file was (re)written. The Python installers
/// rewrite unconditionally and declare idempotency at the "same resulting
/// state" level; skipping the byte-identical rewrite keeps mtime (and any
/// watching daemon) stable without changing the resulting state.
pub fn write_if_changed(path: &std::path::Path, content: &str) -> Result<bool, std::io::Error> {
    // Absent or not text differs from `content`; a file that cannot be read
    // is refused, never written over unseen.
    match std::fs::read_to_string(path) {
        Ok(existing) if existing == content => return Ok(false),
        Ok(_differs) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData
            ) => {}
        Err(error) => {
            return Err(std::io::Error::new(
                error.kind(),
                format!("{} cannot be read: {error}", path.display()),
            ))
        }
    }
    std::fs::write(path, content)?;
    Ok(true)
}
