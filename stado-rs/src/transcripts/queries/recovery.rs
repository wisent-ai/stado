//! Recovery: the newest value for one exact name, and the candidate unlock
//! phrases for a passphrase-protected key. Both consult runtime payloads only,
//! and neither prints anything.

use crate::transcripts::detect::{min_secret_len, pairs_in_line, value_looks_secret};
use crate::transcripts::sources::events::payloads;
use crate::transcripts::sources::files::transcript_files;
use crate::transcripts::Origin;

/// The newest observed value for one exact name, for restoring it into the
/// vault. Separate from [`crate::transcripts::scan`] and per-name on purpose: a
/// caller has to know what it is asking for, and nothing can enumerate values
/// in bulk.
///
/// Only runtime payloads are consulted. A value quoted out of a source file is
/// a literal somebody committed, not the credential the fleet was running with.
pub fn value_for(name: &str) -> Option<String> {
    for path in transcript_files() {
        for (payload, origin) in payloads(&path) {
            if origin != Origin::Runtime {
                continue;
            }
            for line in payload.lines() {
                for (found, value) in pairs_in_line(line) {
                    if found == name && value_looks_secret(&value) {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

/// Longest string still plausible as a passphrase. Beyond this a token is a
/// hash, a bearer, or base64 payload, not something a person or a generator
/// produced as an unlock phrase.
fn max_phrase_len() -> usize {
    "128".parse().unwrap_or_default()
}

/// Candidate unlock phrases for a passphrase-protected key, newest first.
///
/// A protected key is useless without its phrase, and the phrase is the one
/// piece of key material the design deliberately keeps off the machine. When it
/// is nevertheless in a transcript — because some tool printed it, or a process
/// listing caught it — this is where recovery finds it.
///
/// Returns the values in process memory for a caller that tests them. Nothing
/// here prints, and the caller is expected to report which NAME worked rather
/// than what it contained.
pub fn unlock_candidates() -> Vec<(String, String)> {
    let interesting = |name: &str| {
        let upper = name.to_ascii_uppercase();
        upper.contains("UNLOCK") || upper.contains("PASSPHRASE")
    };
    // A phrase also reaches a transcript with no name attached — the output of
    // reading the unlock file is just the phrase on a line by itself. Those
    // payloads are recognised by naming the file, which keeps this narrow: bare
    // tokens are harvested only from output that was demonstrably about the
    // unlock material, never from arbitrary text.
    let about_unlock_file = |payload: &str| {
        payload.contains(".skarbiec-unlock")
            || payload.contains("SKARBIEC_UNLOCK_FILE")
            || payload.contains("skarbiec-unlock")
    };
    let plausible_phrase = |token: &str| {
        let bounds = token.len() >= min_secret_len() && token.len() <= max_phrase_len();
        bounds
            && token.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || c == '+'
                    || c == '/'
                    || c == '='
                    || c == '_'
                    || c == '-'
            })
    };
    let mut candidates: Vec<(String, String)> = Vec::new();
    let push = |name: &str, value: &str, into: &mut Vec<(String, String)>| {
        let known = into
            .iter()
            .any(|(_, seen): &(String, String)| seen == value);
        if !known {
            into.push((name.to_string(), value.to_string()));
        }
    };
    for path in transcript_files() {
        for (payload, origin) in payloads(&path) {
            if origin != Origin::Runtime {
                continue;
            }
            let bare_ok = about_unlock_file(&payload);
            for line in payload.lines() {
                for (name, value) in pairs_in_line(line) {
                    if interesting(&name) && value.len() >= min_secret_len() {
                        push(&name, &value, &mut candidates);
                    }
                }
                if !bare_ok {
                    continue;
                }
                for token in line.split(|c: char| !c.is_ascii_graphic()) {
                    if plausible_phrase(token) {
                        push(
                            "bare token beside an unlock-file mention",
                            token,
                            &mut candidates,
                        );
                    }
                }
            }
        }
    }
    candidates
}
