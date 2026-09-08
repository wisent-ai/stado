//! The inventory: which credential names are recoverable, how often each was
//! seen, how many generations of it survive, and where. Names and counts only.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::transcripts::detect::{name_suggests_secret, one, pairs_in_line, value_looks_secret};
use crate::transcripts::sources::events::payloads;
use crate::transcripts::sources::files::{modified_iso, transcript_files};
use crate::transcripts::{Finding, Origin};

/// Scan the transcripts and report which credential names are recoverable.
/// Returns names and counts only.
///
/// `include_file_quotes` widens the scan to payloads that merely quoted a file.
/// Those are source code, so the names there are usually identifiers rather
/// than credentials in use.
pub fn scan(include_file_quotes: bool) -> Vec<Finding> {
    struct Accumulator {
        occurrences: usize,
        values: Vec<String>,
        newest: String,
        sources: Vec<PathBuf>,
        origin: Origin,
    }
    let mut by_name: BTreeMap<String, Accumulator> = BTreeMap::new();
    for path in transcript_files() {
        let stamp = modified_iso(&path);
        for (payload, origin) in payloads(&path) {
            if origin == Origin::FileQuote && !include_file_quotes {
                continue;
            }
            for line in payload.lines() {
                for (name, value) in pairs_in_line(line) {
                    if !name_suggests_secret(&name) || !value_looks_secret(&value) {
                        continue;
                    }
                    let entry = by_name.entry(name).or_insert_with(|| Accumulator {
                        occurrences: usize::default(),
                        values: Vec::new(),
                        newest: stamp.clone(),
                        sources: Vec::new(),
                        origin,
                    });
                    entry.occurrences = entry.occurrences.saturating_add(one());
                    if !entry.values.contains(&value) {
                        entry.values.push(value);
                    }
                    if !entry.sources.contains(&path) {
                        entry.sources.push(path.clone());
                    }
                    if origin == Origin::Runtime {
                        entry.origin = Origin::Runtime;
                    }
                }
            }
        }
    }
    by_name
        .into_iter()
        .map(|(name, accumulated)| Finding {
            name,
            occurrences: accumulated.occurrences,
            distinct_values: accumulated.values.len(),
            newest_seen: accumulated.newest,
            sources: accumulated.sources,
            origin: accumulated.origin,
        })
        .collect()
}
