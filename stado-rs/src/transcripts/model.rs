//! What a scan reports: where a payload came from, and one credential name
//! as it was observed across the stores.

use std::path::PathBuf;

/// Where a payload came from, which decides whether a match means anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Shell or evaluator output: environments, process tables, command results.
    Runtime,
    /// A file's contents quoted into the transcript by a read or a search.
    FileQuote,
}

/// One credential name observed in the transcripts.
#[derive(Debug, Clone)]
pub struct Finding {
    /// Environment-variable or JSON key the value was written under.
    pub name: String,
    /// How many times the name appeared with a secret-shaped value.
    pub occurrences: usize,
    /// How many DISTINCT values appeared. More than one means the credential
    /// was rotated while transcripts kept every generation, so the newest is
    /// the only one worth restoring.
    pub distinct_values: usize,
    /// Newest file modification time seen, ISO-8601, as the freshness signal.
    pub newest_seen: String,
    /// Files the name appeared in, newest first.
    pub sources: Vec<PathBuf>,
    /// Whether any sighting came from live runtime output.
    pub origin: Origin,
}
