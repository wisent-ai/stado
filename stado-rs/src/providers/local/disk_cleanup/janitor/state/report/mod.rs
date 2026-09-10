//! The report model one pass fills in.

pub(crate) mod build;
pub(crate) mod canonical;
pub(crate) mod sanitize;

use std::collections::BTreeMap;

use crate::providers::local::disk_cleanup::build_caches;

// ---------------------------------------------------------------------------
// report model (Python's nested report dicts)
// ---------------------------------------------------------------------------

/// Python `_cleaner_report()`.
#[derive(Debug, Clone, Default)]
pub struct CleanerReport {
    pub scanned_items: i64,
    pub eligible_items: i64,
    pub deleted_items: i64,
    pub expected_bytes: i64,
    pub actual_free_delta_bytes: i64,
    pub skipped: BTreeMap<String, i64>,
}

/// Python `report["caps"]`.
#[derive(Debug, Clone, Default)]
pub struct Caps {
    pub bytes: bool,
    pub items: bool,
    pub scan: bool,
    pub deadline: bool,
}

impl Caps {
    pub fn any(&self) -> bool {
        self.bytes || self.items || self.scan || self.deadline
    }
}

/// Python `_base_report(...)`.
#[derive(Debug, Clone)]
pub struct CleanupReport {
    pub hostname: String,
    pub target_name: Option<String>,
    pub policy_digest: Option<String>,
    /// Which process made this pass, and the version of the binary that made
    /// it. The state file has several writers on an always-on host; see
    /// [`CleanupWriter`] for why attribution rather than arbitration.
    pub writer: &'static str,
    pub writer_version: &'static str,
    /// True when this host declares no `disk_cleanup` and the reporting
    /// default is in force. An operator reading `mode: report` otherwise
    /// cannot tell a deliberate choice from an absent declaration.
    pub policy_defaulted: bool,
    pub mode: Option<String>,
    pub check_interval_seconds: Option<i64>,
    pub started_at: String,
    pub duration_ms: i64,
    /// Of `duration_ms`, how much was spent waiting on the queue store before
    /// the pass had decided anything — the canonical-registry read and the
    /// workdir keep-list read, both of which happen before the run lock.
    ///
    /// `duration_ms` said 818021 and `outcome` said `healthy_noop`, and
    /// between those two numbers there was no way to tell a janitor that had
    /// walked a very large filesystem from one that had waited a quarter of an
    /// hour on a network read for a keep-list no cleaner on that pass would
    /// ever consult. Those call for opposite responses — one is the host, the
    /// other is ours — and every consumer of this report was being handed the
    /// verdict without the cost's shape. `scanned`/`cleaners: null` already
    /// says a pass reached no cleaner; this says where such a pass spent its
    /// time, so `healthy_noop` can never again hide a wait inside a word that
    /// means "nothing needed doing".
    pub store_wait_ms: i64,
    pub outcome: String,
    pub free_bytes_before: Option<i64>,
    pub free_bytes_after: Option<i64>,
    pub low_bytes: Option<i64>,
    pub target_bytes: Option<i64>,
    pub pressure_active: Option<bool>,
    pub hf: CleanerReport,
    pub weles: CleanerReport,
    pub builds: CleanerReport,
    pub clones: CleanerReport,
    pub workdirs: CleanerReport,
    pub backup_twins: CleanerReport,
    pub release_store: CleanerReport,
    pub caps: Caps,
    pub lock_busy: bool,
    pub active_job_count: i64,
    pub last_success_at: Option<String>,
    /// Whether this pass reached its cleaners at all.
    ///
    /// Set once, immediately before the first cleaner runs. It exists because
    /// the report used to carry a complete cleaner table of zeros no matter
    /// how early the pass gave up, and a table of zeros is byte-for-byte what
    /// a successful pass that found nothing to delete emits. On
    /// `lukasz-macbook` that made 12,197 records over fifteen days — 8,539 of
    /// them `invalid_or_unavailable_policy` and 2,030 `lock_busy`, neither of
    /// which resolved a policy or opened a single directory — indistinguishable
    /// from fifteen days of "nothing needed doing", which is why nobody
    /// noticed the janitor had never once deleted anything.
    ///
    /// A pass that did not reach its cleaners now emits `cleaners: null`
    /// rather than a measurement it never made. Both readers of the table
    /// already tolerate its absence
    /// ([`crate::deploy::host_state::cleanup::cleaner_plans`] returns no rows and
    /// `stado space report` keeps janitor state separate), and the `outcome`
    /// vocabulary is unchanged: `lock_busy`, `interval_noop`,
    /// `invalid_or_unavailable_policy` and `healthy_noop` already say which
    /// non-run this was.
    pub scanned: bool,
    /// Declared cleaners this pass never scanned, because the scan share or
    /// the pass deadline was spent before their turn came.
    ///
    /// [`scanned`](Self::scanned) says whether a pass reached its cleaners at
    /// all; this says which of them it never reached, and it exists for the
    /// same reason: the table publishes `scanned 0, eligible 0, deleted 0`
    /// for a cleaner that was never given a turn, which is byte-for-byte what
    /// a cleaner that looked and found nothing emits. On `charless-mac-mini`
    /// the cleaners run in a fixed order with `backup_twins` last, the policy
    /// declared no `max_pass_seconds` so every pass took the janitor's own 30
    /// seconds against a `$HOME` holding 103.9 GiB under `~/.stado` alone,
    /// and `build_caches` — which walks all of `$HOME` by design — ended the
    /// pass inside itself. `backup_twins` reported zeros with
    /// `skipped {scan_cap: 1, scan_deadline: 1}` for as long as anyone had
    /// looked, under real pressure, while the host refused every ordinary job
    /// for eleven days. The outcome was `cap_reached`, which is true, names
    /// the budget and not the cleaner, and reads like a finished look at the
    /// disk.
    ///
    /// Empty when every declared cleaner had its turn, so a reader can tell
    /// "nothing was eligible" from "nobody looked".
    pub unscanned_cleaners: Vec<String>,
    /// Cleaners the policy names that this binary does not implement: the
    /// registry is read by every release at once, and a name a newer release
    /// knows is not a reason to run none of the ones this release knows.
    pub unknown_cleaners: Vec<String>,
    /// Human-readable position of the next build-cache visit. New passes
    /// derive this from `builds_cursor`; legacy reports retain it for display.
    pub builds_resume_from: Option<String>,
    /// The authoritative checkpoint, including all unvisited directories.
    pub(in crate::providers::local::disk_cleanup) builds_cursor:
        Option<build_caches::BuildCachesCursor>,
    pub(in crate::providers::local::disk_cleanup) backup_cursor:
        Option<crate::providers::local::disk_cleanup::backup_twins::cursor::BackupCursor>,
    pub errors: Vec<String>,
}
