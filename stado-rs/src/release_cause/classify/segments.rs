//! Turning a recorded reason back into the lines a human wrote, so the one
//! line that names the cause can be quoted on its own.

/// The evidence line kept beside the cause.
///
/// Wide enough for every decisive sentence observed on the fleet — the longest
/// is a `capability-issue` refusal naming a resource, a coordinate and its
/// remedy, at about a hundred and sixty characters — and narrow enough that one
/// row of `release doctor` stays one row. The reason string keeps the full
/// (truncated) tail; this is the one line that earned the name.
const EVIDENCE_CHARS: usize = 240;

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

/// The segments a decisive line could be.
///
/// Three separators, each one this crate writes itself. A quarantine reason is
/// composed as `<symptom>; stderr <path>: <tail>; stdout <path>: <tail>`, and
/// each tail is a log tail joined with `" | "`. Splitting on real newlines
/// alone would quote the whole reason back as one "line", which is what the
/// operator was already staring at.
fn segments(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .flat_map(|line| line.split(" | "))
        .flat_map(|part| part.split("; "))
        .map(unlabel)
}

/// Drop the `stderr <path>: ` label the reason puts in front of the first line
/// of a quoted tail.
///
/// Written by [`crate::release_agent`] one line above where the tail is joined,
/// so this removes a known prefix rather than guessing at one. Without it the
/// evidence for a record whose decisive sentence is the first line of its
/// stderr is that sentence with a file path bolted to the front, and the bound
/// then spends a third of its width on the path.
fn unlabel(segment: &str) -> &str {
    let trimmed = segment.trim();
    for label in ["stderr ", "stdout "] {
        if let Some(rest) = trimmed.strip_prefix(label) {
            // `<path>: <line>` — the first `": "` ends the path. A bracketed
            // note ("[... is empty]") carries no such separator and is left
            // whole, because the note IS the whole answer in that case.
            if let Some((_, line)) = rest.split_once(": ") {
                return line.trim();
            }
        }
    }
    trimmed
}

/// The narrowest segment carrying one of `needles`, bounded and trimmed.
///
/// Narrowest rather than first: a reason's opening segment is the symptom, and
/// on a legacy record the first log line is glued to it, so "first match" hands
/// back the sentence this module exists to stop quoting. The shortest segment
/// that contains the match is the line that carries it and little else.
///
/// Falls back to the whole (bounded) text when no single segment holds the
/// match, which happens when a needle straddles a join. Reporting the match
/// without the line it came from would leave the operator with a name and no
/// quotation.
pub(super) fn evidence_for(text: &str, needles: &[&str]) -> String {
    let found = segments(text)
        .filter(|segment| {
            let lowered = segment.to_lowercase();
            needles.iter().any(|needle| lowered.contains(needle))
        })
        .min_by_key(|segment| segment.chars().count())
        .unwrap_or_else(|| text.trim());
    bound(found)
}

pub(in crate::release_cause) fn bound(text: &str) -> String {
    match text.char_indices().nth(EVIDENCE_CHARS) {
        None => text.to_string(),
        Some((cut, _)) => format!("{}…", &text[..cut]),
    }
}

pub(super) fn matches_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}
