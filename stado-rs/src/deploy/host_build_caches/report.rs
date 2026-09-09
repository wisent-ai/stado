//! What one cache pass reports: the per-directory verdicts, one host's
//! outcome, the declaration that drove it, and the reader of the marked lines.

use super::program::STATUS_PREFIX;

/// One reported directory: what happened to it, where, and its size in KiB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub state: String,
    pub path: String,
    pub kib: String,
}

/// One host's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheReport {
    pub target: String,
    pub entries: Vec<CacheEntry>,
    pub error: Option<String>,
}

/// The resolved registry declaration that drove one cache read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheDeclaration {
    pub root: String,
    pub min_age_seconds: i64,
}

pub fn parse_report(stdout: &str) -> Vec<CacheEntry> {
    let mut entries = Vec::new();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix(STATUS_PREFIX) else {
            continue;
        };
        let mut fields = rest.split('\t');
        let Some(state) = fields.next().filter(|state| !state.is_empty()) else {
            continue;
        };
        let path = fields.next().unwrap_or_default();
        let kib = fields.next().unwrap_or_default();
        entries.push(CacheEntry {
            state: state.to_string(),
            path: path.to_string(),
            kib: kib.to_string(),
        });
    }
    entries
}
