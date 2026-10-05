//! The report model one pass fills in.

pub(crate) mod build;
pub(crate) mod canonical;
pub(crate) mod sanitize;

use std::collections::BTreeMap;

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
    /// The bytes each skip reason kept, where the cleaner knows them.
    ///
    /// A count answers "how many files did you leave"; an operator looking
    /// at a host still full is asking "where are the bytes". A cleaner can
    /// report thousands of `record_kept` and nothing eligible beside many
    /// GiB, and the counts cannot say whether the bytes are in the records or
    /// somewhere the pass never reached.
    pub skipped_bytes: BTreeMap<String, i64>,
}

/// Python `_base_report(...)`.
#[derive(Debug, Clone)]
pub struct CleanupReport {
    pub hostname: String,
    pub target_name: Option<String>,
    /// Which process made this pass, and the version of the binary that made
    /// it. The state file has several writers on an always-on host; see
    /// [`CleanupWriter`] for why attribution rather than arbitration.
    pub writer: &'static str,
    pub writer_version: &'static str,
    /// The period the writer makes passes at, when it makes more than one;
    /// `None` for a single pass. The next pass is promised from it.
    pub every_seconds: Option<u64>,
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
    /// The volume's size when the pass read it, beside the free bytes.
    pub total_bytes: Option<i64>,
    /// Percent used before and after the cleaners, as the rule judges it.
    pub used_percent_before: Option<f64>,
    pub used_percent_after: Option<f64>,
    /// Whether the volume was at the rule's threshold when the pass began.
    pub pressure_active: Option<bool>,
    pub hf: CleanerReport,
    pub weles: CleanerReport,
    pub builds: CleanerReport,
    pub clones: CleanerReport,
    pub workdirs: CleanerReport,
    pub job_outputs: CleanerReport,
    pub backup_twins: CleanerReport,
    pub release_store: CleanerReport,
    /// Local Time Machine snapshots. On a Mac this is what decides whether
    /// anything the other cleaners removed became free space.
    pub local_snapshots: CleanerReport,
    pub object_evidence: CleanerReport,
    /// Logs of the coding-agent harnesses under this account's home.
    pub agent_logs: CleanerReport,
    pub lock_busy: bool,
    pub active_job_count: i64,
    pub last_success_at: Option<String>,
    /// Whether this pass reached its cleaners at all.
    ///
    /// Set once, immediately before the first cleaner runs. It exists because
    /// the report used to carry a complete cleaner table of zeros no matter
    /// how early the pass gave up, and a table of zeros is byte-for-byte what
    /// a successful pass that found nothing to delete emits. Thousands of
    /// `lock_busy` records, none of which opened a single directory, would be
    /// indistinguishable from weeks of "nothing needed doing", and nobody
    /// would notice the janitor had never once deleted anything.
    ///
    /// A pass that did not reach its cleaners emits `cleaners: null` rather
    /// than a measurement it never made, and its `outcome` (`lock_busy`,
    /// `healthy_noop`, `volume_unreadable`) says which non-run this was.
    pub scanned: bool,
    pub errors: Vec<String>,
}
