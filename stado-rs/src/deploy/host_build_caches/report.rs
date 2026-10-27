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
    /// The walk was killed by its own budget and nothing else went wrong.
    ///
    /// Kept apart from `error` because the two ask for opposite things from a
    /// caller. A host that refused the read, or has no path to it, has told
    /// the operator nothing and the command must fail. A walk that ran out of
    /// seconds over a large declared root has already produced every other
    /// figure in the report, and failing the whole command then throws away
    /// the free space, the watermarks, the janitor's last pass and the
    /// coverage after printing all of them.
    pub timed_out: bool,
}

impl BuildCacheReport {
    /// The build-output census `space report` prints beside the verdicts,
    /// taken from the same walk: every directory the walk sized (a row whose
    /// size is a number of KiB — the program sizes every tagged directory,
    /// judged or not, and prints `-` for every other row), and whether the
    /// walk ran to its end, which an empty census must say apart from one
    /// nobody finished. A refused directory leaves its contents unknown,
    /// even if the walk found readable caches elsewhere.
    pub fn census(&self) -> (Vec<crate::deploy::host_disk::DiskItem>, bool) {
        let rows = self
            .entries
            .iter()
            .filter_map(|entry| match entry.kib.parse::<i64>() {
                Ok(blocks_kb) => Some(crate::deploy::host_disk::DiskItem {
                    blocks_kb,
                    path: entry.path.clone(),
                }),
                // The program prints `-` for a row it did not size.
                Err(_not_a_size) => None,
            })
            .collect();
        let walked = !self.entries.iter().any(|entry| {
            entry.state == super::program::SCAN_FAILED
                || entry.state == super::program::ROOT_ABSENT
                || entry.state == "permission-denied"
        });
        (rows, walked && self.error.is_none() && !self.timed_out)
    }
}

/// The resolved registry declaration that drove one cache read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheDeclaration {
    pub root: String,
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
        // A status line without its path or size is a walk that did not
        // report what it saw: recorded as a failed scan carrying the line, so
        // the census is incomplete, never a cache silently left out.
        let (Some(path), Some(kib)) = (
            fields.next().filter(|path| !path.is_empty()),
            fields.next().filter(|kib| !kib.is_empty()),
        ) else {
            entries.push(CacheEntry {
                state: super::program::SCAN_FAILED.to_string(),
                path: format!("status line without a path and size: {rest}"),
                kib: String::new(),
            });
            continue;
        };
        entries.push(CacheEntry {
            state: state.to_string(),
            path: path.to_string(),
            kib: kib.to_string(),
        });
    }
    entries
}
