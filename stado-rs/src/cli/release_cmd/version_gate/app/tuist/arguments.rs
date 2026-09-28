//! Argument lists of the manifest's calls: `label: value` pairs, and the
//! argument text of `.member(...)`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::text::{anchored, balanced, excerpt, skip_trivia, split_top_level, trimmed, Read};

const STEP: usize = 1;
/// Where an argument's text starts.
const TEXT_START: usize = 0;

static ARGUMENT_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<member>[A-Za-z_][A-Za-z0-9_]*)\s*:").expect("valid"));
static CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.\s*(?P<member>[A-Za-z_][A-Za-z0-9_]*)\s*\(").expect("valid"));
pub(crate) static MEMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.\s*(?P<member>[A-Za-z_][A-Za-z0-9_]*)\s*$").expect("valid"));

/// The member name when `text` is a bare `.member`.
pub(crate) fn member(text: &[char]) -> Option<String> {
    anchored(&MEMBER, text).map(|(name, _)| name)
}

/// `label: value` pairs of one argument list. Positional arguments are ignored.
pub(crate) fn labelled(text: &[char], origin: &str) -> Read<BTreeMap<String, Vec<char>>> {
    let mut found = BTreeMap::new();
    for piece in split_top_level(text, origin)? {
        let start = skip_trivia(&piece, TEXT_START, origin)?;
        let body = trimmed(&piece[start..]);
        let Some((label, end)) = anchored(&ARGUMENT_LABEL, body) else {
            continue;
        };
        if found.contains_key(&label) {
            return Err(format!("{origin}: argument '{label}' appears twice"));
        }
        found.insert(label, trimmed(&body[end..]).to_vec());
    }
    Ok(found)
}

/// The argument text of `.member(...)`, or `None` when `text` is another shape.
pub(crate) fn call(text: &[char], member: &str, origin: &str) -> Read<Option<Vec<char>>> {
    match anchored(&CALL, text) {
        Some((found, _)) if found == member => {}
        _ => return Ok(None),
    }
    let opening = text
        .iter()
        .position(|&character| character == '(')
        .expect("the call pattern matched an opening parenthesis");
    let end = balanced(text, opening, origin)?;
    if !trimmed(&text[end..]).is_empty() {
        return Err(format!(
            "{origin}: trailing text after .{member}(...): {:?}",
            excerpt(text, end)
        ));
    }
    Ok(Some(text[opening + STEP..end - STEP].to_vec()))
}

/// The value text of `label`, or an error naming what is missing.
pub(crate) fn required<'a>(
    arguments: &'a BTreeMap<String, Vec<char>>,
    label: &str,
    origin: &str,
    what: &str,
) -> Read<&'a [char]> {
    arguments
        .get(label)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{origin}: {what} has no {label}"))
}

/// Shown in errors about a value this reader cannot read.
pub(crate) fn spelled(text: &[char]) -> String {
    excerpt(text, TEXT_START)
}
