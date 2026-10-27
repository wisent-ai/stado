//! Which assignment of a key wins, and which keys the file assigns twice.
//!
//! A sourced file assigns top to bottom, so precedence is file order and
//! nothing else. Both spellings of an assignment count, because both of them
//! assign.

use std::collections::BTreeMap;

use super::super::*;

/// One entry's role among every assignment to the same key, in file order:
/// [`EFFECTIVE`] for the last one, [`SHADOWED`] for every earlier one.
///
/// This is the finding the whole command exists for. A sourced file assigns
/// top to bottom, so the LAST assignment is the one the process runs with, and
/// an operator reading a `KEY=` near the top of the file is reading dead text.
/// Both spellings count: `export KEY=…` assigns exactly like `KEY=…`, and
/// `env set`'s `^KEY=` rewrite cannot see the export form at all — so a file
/// can hold an `export` line the writer will never replace.
pub fn shadowing(entries: &[EnvEntry]) -> Vec<&'static str> {
    let mut last: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.form == FORM_UNPARSABLE || entry.key.is_empty() {
            continue;
        }
        last.insert(entry.key.as_str(), index);
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if entry.form == FORM_UNPARSABLE || entry.key.is_empty() {
                return "";
            }
            if last.get(entry.key.as_str()) == Some(&index) {
                EFFECTIVE
            } else {
                SHADOWED
            }
        })
        .collect()
}

/// Every key the file assigns more than once, in first-appearance order.
pub fn duplicate_keys(entries: &[EnvEntry]) -> Vec<String> {
    // The first entry of each key, kept so a duplicate is recognised when
    // the key is seen again; keys are reported in first-appearance order.
    let mut first_seen: BTreeMap<&str, ()> = BTreeMap::new();
    let mut duplicates: Vec<String> = Vec::new();
    for entry in entries {
        if entry.form == FORM_UNPARSABLE || entry.key.is_empty() {
            continue;
        }
        if first_seen.insert(entry.key.as_str(), ()).is_some()
            && !duplicates.iter().any(|key| key == &entry.key)
        {
            duplicates.push(entry.key.clone());
        }
    }
    duplicates
}
