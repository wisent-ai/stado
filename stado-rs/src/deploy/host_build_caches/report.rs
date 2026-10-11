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
    /// nobody finished: a walk whose root was absent or that `find` ended
    /// without a tag saw nothing, whatever its exit status.
    pub fn census(&self) -> (Vec<crate::deploy::host_disk::DiskItem>, bool) {
        let rows =
            self.entries
                .iter()
                .filter_map(|entry| {
                    entry.kib.parse::<i64>().ok().map(|blocks_kb| {
                        crate::deploy::host_disk::DiskItem {
                            blocks_kb,
                            path: entry.path.clone(),
                        }
                    })
                })
                .collect();
        let walked = !self.entries.iter().any(|entry| {
            entry.state == super::program::SCAN_FAILED || entry.state == super::program::ROOT_ABSENT
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
