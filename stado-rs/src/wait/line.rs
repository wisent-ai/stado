//! The text of one wait line: its fields, its clock readings and the write.

use std::io::Write;
use std::time::Instant;

/// A value made safe for a field: `;` separates fields and a line break
/// ends the line, so neither may appear inside one.
pub(crate) fn field(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| match character {
            ';' => ',',
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

/// This moment in UTC, to the millisecond, as RFC 3339.
pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Seconds since `started`, to the millisecond.
pub(crate) fn seconds(started: Instant) -> String {
    format!("{:.3}", started.elapsed().as_secs_f64())
}

/// An error and every source under it, outermost first. A source whose text
/// the outer message already carries is not repeated.
pub(crate) fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut next = error.source();
    while let Some(source) = next {
        let said = source.to_string();
        if !said.is_empty() && !text.contains(&said) {
            text.push_str(": ");
            text.push_str(&said);
        }
        next = source.source();
    }
    text
}

/// One line on stderr, in a single write. A stderr that cannot be written
/// has nowhere to say so.
pub(crate) fn write(line: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(format!("{line}\n").as_bytes());
}
