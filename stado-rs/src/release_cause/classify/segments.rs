//! Cleaning the line kept as a cause's evidence.

/// What is left to scan once truncation has cut a tail off inside an escape
/// sequence: nothing, in both of the ways that can happen.
const NOTHING_LEFT: &str = "";

/// Drop terminal control sequences.
///
/// Products on this fleet log through `tracing`'s ANSI writer, so a decisive
/// line arrives wrapped in colour escapes. They are removed before matching
/// (an escape sitting inside a phrase would hide it) and before the evidence is
/// stored (a report is read in a table, not a terminal emulator). Borrowed
/// unchanged when there is nothing to strip, which is every reason the agent
/// writes itself.
///
/// A CSI sequence is `ESC [`, then parameter and intermediate bytes, then one
/// final byte in `@`..=`~`. The introducer `[` is itself inside that range, so
/// it has to be stepped over explicitly — scanning for the final byte from
/// directly after the escape terminates on the `[` and leaves `2m` behind in
/// the evidence, which is exactly what the first run of this against the live
/// host printed.
pub(super) fn strip_ansi(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\u{1b}') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('\u{1b}') {
        out.push_str(&rest[..start]);
        let after_escape = &rest[start + '\u{1b}'.len_utf8()..];
        let Some(introducer) = after_escape.chars().next() else {
            // A lone escape at the end of a truncated tail. Dropped.
            rest = NOTHING_LEFT;
            break;
        };
        if introducer != '[' {
            // A two-character escape (charset selection and friends). Drop
            // both and carry on rather than swallowing the rest of the line.
            rest = &after_escape[introducer.len_utf8()..];
            continue;
        }
        let body = &after_escape[introducer.len_utf8()..];
        match body.char_indices().find(|(_, ch)| ('@'..='~').contains(ch)) {
            Some((end, final_byte)) => rest = &body[end + final_byte.len_utf8()..],
            // Truncation cut the sequence before its final byte; there is no
            // text left in it to keep.
            None => {
                rest = NOTHING_LEFT;
                break;
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// The evidence kept beside a cause: the first line of `text`, whole, so one
/// row of `release doctor` stays one row without cutting the sentence that
/// earned the name.
pub(in crate::release_cause) fn evidence_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_string()
}
