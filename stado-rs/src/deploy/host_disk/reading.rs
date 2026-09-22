//! Parsing one host's answer: the reading types and the marker fold.

use super::*;

mod fold;

pub use fold::{parse_lsblk_pairs, parse_output};

/// `df -Pk` 1024-byte blocks as GiB, one decimal.
///
/// This module owns the unit because it owns the `df` invocation, and both
/// [`crate::deploy::host_gates`] and [`crate::deploy::host_reclaim`] report
/// free space in GiB against a registry policy that declares its watermarks in
/// GiB (`low_free_gb * `[`disk_cleanup::GIB`]). Three spellings of the same
/// division would eventually be three different numbers on one host.
pub fn gib_from_blocks(blocks: f64) -> f64 {
    (blocks / (1024.0 * 1024.0) * 10.0).round() / 10.0
}

/// One `df -Pk` row, in the units the host reported (1024-byte blocks for
/// the three sizes, a percentage string for capacity).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskUsage {
    pub filesystem: String,
    pub blocks_kb: String,
    pub used_kb: String,
    pub available_kb: String,
    pub capacity: String,
    pub mounted_on: String,
}

/// One block device as the host's `lsblk -b -P` named it: a disk, a
/// partition or a mapper volume, with the filesystem on it and where it is
/// mounted, when it is. `mountpoint` empty on a `disk` or `part` with no
/// children is the fact this record exists for: storage the host has and
/// the fleet cannot write to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockDevice {
    pub name: String,
    pub size_bytes: i64,
    pub kind: String,
    pub fstype: String,
    pub mountpoint: String,
    pub uuid: String,
    pub model: String,
}

impl BlockDevice {
    /// A whole disk or partition holding no mounted filesystem, no partition
    /// of its own that is listed, and no filesystem that is in use without
    /// a mountpoint (an LVM physical volume, a LUKS container, swap):
    /// attached, and unused by anything the kernel mounts.
    pub fn unmounted_among(&self, all: &[BlockDevice]) -> bool {
        if (self.kind != "disk" && self.kind != "part")
            || !self.mountpoint.is_empty()
            || self.size_bytes <= 0
        {
            return false;
        }
        if matches!(self.fstype.as_str(), "LVM2_member" | "crypto_LUKS" | "swap") {
            return false;
        }
        !all.iter().any(|other| {
            other.name != self.name
                && other.name.starts_with(&self.name)
                && (other.kind == "part" || !other.mountpoint.is_empty())
        })
    }
}

/// What the host's janitor state file says about the last and next pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CleanupState {
    /// Absent when the host has no state file at all — a host whose
    /// janitor has never completed a pass, which is itself the finding.
    pub present: bool,
    /// Where the state file was looked for, when it was not there.
    pub path: Option<String>,
    pub last_pass_at: Option<String>,
    pub last_success_at: Option<String>,
    /// When a pass was last PREVENTED from running because a workload held
    /// the run lock in shared mode, from the janitor's own
    /// `last_prevented_at`.
    ///
    /// The distinction `last_success_at` alone cannot draw. A janitor that is
    /// being prevented is healthy and blocked; a janitor that is silent is
    /// broken or unscheduled; and before this was recorded both read as the
    /// same absence, so a host running one long job looked exactly like a host
    /// whose janitor had died. `None` on a host that has never been prevented,
    /// and on any host whose janitor predates the stamp.
    pub last_prevented_at: Option<String>,
    pub outcome: Option<String>,
    /// Which process wrote the report this reading came from, and the version
    /// of the binary that wrote it.
    ///
    /// The state file has several writers on an always-on host: the queue agent
    /// every tick, and a `disk-cleanup --watch` unit on its own timer. On
    /// 2026-08-31 the agent reported `interval_noop` with no errors at
    /// 14:55:24Z and this command read `invalid_or_unavailable_policy` from the
    /// same path 46 seconds later. Both readings were true about their own
    /// writer and neither was true about the host, so `outcome` alone told an
    /// operator whichever answer arrived last.
    ///
    /// Reporting it does not arbitrate. It makes the reading say whose verdict
    /// it is, which is the difference between a fact and a coin toss.
    pub writer: Option<String>,
    pub writer_version: Option<String>,
    /// The pid that wrote the pass. First shipped in stado 0.13.24 (#232);
    /// a file written by anything older carries no author at all, which is the
    /// state this field exists to end.
    ///
    /// This said "present from stado 0.13.20", and charless-mac-mini disproved
    /// it on 2026-09-01: a pass written at 17:02:38Z carried
    /// `writer: "agent-tick"`, `writer_version: "0.13.20"` and
    /// `writer_pid: null`. `git merge-base --is-ancestor` puts #232 in
    /// `stado-v0.13.24` and in no tag before it.
    ///
    /// And a version number is not the whole answer to "who wrote this".
    /// Alternating passes on that same host carried no `writer` field at all,
    /// and the writer turned out to be
    /// `python3.12 -m stado.cli agent --target charless-mac-mini` under
    /// `com.stado.agent.charless-mac-mini` — the Python agent, not this binary.
    /// No release of this crate will ever make that process stamp a pid,
    /// because it does not run this code. Delivering a newer `stado` does not
    /// change what an already-running process executes either: the file on
    /// disk is replaced, the process keeps its own image until it re-execs.
    pub writer_pid: Option<i64>,
    pub free_bytes_before: Option<i64>,
    pub free_bytes_after: Option<i64>,
    /// `free_bytes_after - free_bytes_before` of the recorded pass. Free
    /// space can fall during a pass while other processes write, so this
    /// is signed and reported as measured rather than clamped.
    pub freed_bytes: Option<i64>,
    pub next_pass_at: Option<String>,
    /// The low watermark the janitor VALIDATED on its last pass, in bytes, or
    /// `None` when the recorded report does not identify a canonical policy.
    ///
    /// Read through the janitor's own
    /// [`disk_cleanup::validated_report_low_bytes`], which is the same
    /// function the queue agent resolves `disk_low_bytes` with. It matters
    /// here because it, and not the registry declaration, is the number
    /// admission is gated on when a host cannot read the registry — the Mac
    /// mini published `disk_pressure_unresolved` for hours and the CLI could
    /// not show what threshold that verdict was measured against.
    pub low_bytes: Option<i64>,
    /// The state document was there but did not parse.
    pub error: Option<String>,
    /// The pass as recorded by its writer, including per-cleaner refusals and
    /// exhausted limits. Directory sizes cannot explain why a pass stopped.
    pub report: Option<Value>,
}

/// The local APFS snapshots this host is holding, which nothing in this
/// product removes.
///
/// Their blocks are inside `df`'s used figure, so free space does not come
/// back until they are thinned. `stado space reclaim` handles them only in its
/// declared APFS stage and only against the target watermark, because dropping
/// a snapshot is a decision about that machine's recovery rather than ordinary
/// janitor cleanup. Reported so nobody reads a reclamation that freed nothing
/// and concludes the space is unexplained.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalSnapshots {
    /// Whether the host could be asked at all. False on every Linux host and
    /// on any Mac without `tmutil`: "nobody looked" is not "there are none".
    pub supported: bool,
    /// The snapshot names as the host listed them, verbatim — the same
    /// strings `tmutil deletelocalsnapshots` and `tmutil thinlocalsnapshots`
    /// take, so what is printed here is what an operator can act on.
    ///
    /// No sizes: macOS publishes none for a snapshot (see the module header),
    /// and a byte figure derived from anything else here would be a guess
    /// wearing a number's clothes.
    pub names: Vec<String>,
}

/// One measured directory in the bounded host inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskItem {
    pub blocks_kb: i64,
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloneSummary {
    pub path: String,
    pub total: i64,
    pub older_than_hour: i64,
    pub older_than_day: i64,
}

/// One process holding the janitor's run lock, as the host's `lsof` named it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockHolder {
    pub pid: String,
    pub command: String,
}

/// What the host has left of its memory, as its own kernel reports it.
///
/// The same figures the host's own memory pass measures itself against
/// ([`crate::providers::local::host_memory::reading::MemoryReading`]),
/// gathered here over the control channel for a host this process is not
/// running on. The parsers are that module's, not a second implementation:
/// a report that disagreed with the pass about how much memory a host has
/// would make every watermark verdict arguable.
pub type MemoryReading = crate::providers::local::host_memory::reading::MemoryReading;

/// Everything one host answered.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskReading {
    pub usage: Option<DiskUsage>,
    /// Every device-backed filesystem `df -Pk` listed, the fleet's volume
    /// among them.
    pub volumes: Vec<DiskUsage>,
    /// Every block device the kernel sees, mounted or not. Empty with
    /// `block_devices_read` false means the host has no `lsblk` (macOS).
    pub block_devices: Vec<BlockDevice>,
    pub block_devices_read: bool,
    pub clone_summaries: Vec<CloneSummary>,
    pub clone_root: Option<String>,
    pub state: CleanupState,
    pub snapshots: LocalSnapshots,
    pub inventory: Vec<DiskItem>,
    /// Every directory a build tool tagged regenerable, from the census that
    /// is not bounded by the inventory's depth. Kept apart from `inventory`
    /// until the nested ones are folded away, because two tagged trees, one
    /// inside the other, would otherwise be counted twice.
    pub tagged_build_caches: Vec<DiskItem>,
    /// The census ran to its end. Empty rows with this false means nobody
    /// looked, which is a different fact from a host that holds no build
    /// output and must never be reported as the same one.
    pub tagged_build_caches_read: bool,
    /// Who holds the run lock right now. Empty with `lock_read` true means
    /// nothing holds it, which is a different fact from never having looked.
    pub lock_holders: Vec<LockHolder>,
    pub lock_read: bool,
    pub lock_path: Option<String>,
    pub memory: MemoryReading,
    /// The memory pass's own state document, verbatim, or `null` when the
    /// host has never run one.
    pub memory_state: Value,
}

/// Epoch seconds as the ISO-8601 spelling the rest of the fleet uses.
pub(super) fn iso_from_epoch(epoch: f64) -> Option<String> {
    DateTime::from_timestamp(epoch.trunc() as i64, u32::default()).map(crate::models::isoformat_utc)
}
