//! The registry declaration a host is measured against: `targets[].memory_reclaim`.
//!
//! The disk twin is [`crate::targets::DiskCleanupPolicy`], and this is
//! deliberately the same shape — a mode, an interval, two watermarks, a
//! per-pass budget, and a map whose KEYS are the only work the pass is
//! permitted to do. Nothing here is keyed to a process name or a unit label
//! written in code: the registry names the repairs, and a host that declares
//! none gets a pass that reads its memory and changes nothing.
//!
//! The type lives beside the pass rather than in `targets.rs` for the same
//! reason `ComputeTarget::display_stream` carries
//! `crate::stream::schema::DisplayStream`: the subsystem owns its own schema
//! and the registry struct names it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::providers::local::host_memory::constants;

/// Restart a declared OS unit whose process cannot allocate.
pub const REPAIR_RESTART_UNIT: &str = "restart_unit";
/// Run a declared host-recovery program that reaps its own stale daemons.
pub const REPAIR_REAP_RECOVERY: &str = "reap_recovery";
/// Terminate a named process of the logged-in graphical session.
pub const REPAIR_GRAPHICAL_SESSION: &str = "graphical_session";

/// Every repair name a declaration may carry, in the order a pass tries them.
///
/// Order is narrowest-first and is part of the contract: a unit Stado
/// declared is restarted before a recovery program is run, and a graphical
/// session process is only ever reached last, because it belongs to a person
/// who is logged in.
pub const REPAIR_NAMES: [&str; 3] = [
    REPAIR_RESTART_UNIT,
    REPAIR_REAP_RECOVERY,
    REPAIR_GRAPHICAL_SESSION,
];

/// One permitted repair's policy.
///
/// The twin of [`crate::targets::DiskCleanerPolicy`], including its rule that
/// a field which authorizes irreversible work is a separate opt-in rather
/// than an implication of declaring the cleaner at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRepairPolicy {
    /// `restart_unit`: the launchd labels or systemd unit names this host
    /// permits a restart of. A repair that names none is a declaration with
    /// no effect and validation refuses it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub units: Vec<String>,
    /// `graphical_session`: the process names this host permits termination
    /// of, and only when `allow_graphical_session` is also true.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<String>,
    /// `reap_recovery`: the declared host-recovery program to run, by name —
    /// `recover-skarbiec-crypto` is the one this fleet already has, and it is
    /// what reaps the stale GnuPG daemons (`keyboxd` was holding 211 MB on
    /// charless-mac-mini).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    /// How long a process must have been running before this repair may
    /// touch it. Absent means the pass applies no age rule of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_age_seconds: Option<i64>,
    /// Explicit opt-in to terminate a process of the logged-in graphical
    /// session. False, and false is the whole point: declaring
    /// `graphical_session` makes the pass REPORT which declared processes it
    /// would terminate and how much they hold; it never signals one until an
    /// operator writes this flag. Killing somebody's Safari is not a
    /// consequence of declaring an interest in memory.
    ///
    /// Written only where it is true, so a `restart_unit` or `reap_recovery`
    /// declaration does not carry an authorization for work it cannot do.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_graphical_session: bool,
}

/// A `false` flag is an absent flag in a declaration.
fn is_false(value: &bool) -> bool {
    !*value
}

/// Memory-reclaim policy for a local target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReclaimPolicy {
    /// `off`, `report` or `enforce`; only `enforce` repairs anything.
    pub mode: String,
    pub check_interval_seconds: i64,
    /// Available memory below this, in MiB, is pressure.
    pub low_free_mb: i64,
    /// A pass stops as soon as this much, in MiB, is available.
    pub target_free_mb: i64,
    /// Swap utilisation at or above this percentage is pressure on its own,
    /// whatever the free-memory reading says.
    pub high_swap_used_pct: i64,
    /// How many repairs one pass may perform.
    pub max_repairs_per_pass: i64,
    /// Publish this host as not accepting jobs while it is over its
    /// watermark. Declared, never inferred: a host may be worth reporting on
    /// and still be the only machine that can run the work.
    #[serde(default)]
    pub refuse_placement: bool,
    /// Seconds one pass may spend. Absent means
    /// [`constants::PASS_DEADLINE_SECONDS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pass_seconds: Option<i64>,
    /// The repairs this host permits, by name. An empty map is a host that
    /// reports its memory and repairs nothing.
    pub repairs: BTreeMap<String, MemoryRepairPolicy>,
}

impl MemoryReclaimPolicy {
    /// What a `local` target that declares no `memory_reclaim` is measured
    /// against.
    ///
    /// The disk twin is [`crate::targets::DiskCleanupPolicy::reporting_default`]
    /// and the judgement is the same one, taken for the same reason: before
    /// it existed an undeclared host was not a host with a lenient policy, it
    /// was a host nothing looked at. charless-mac-mini declared no memory
    /// anything, so when its pre-check runner's listener died with `Failed to
    /// create CoreCLR, HRESULT: 0x8007000C` and exit 137 on 2026-09-06, the
    /// fleet's own read path could report the host's disk, its units and its
    /// uptime, and had no field in which to say that 1.3 GB of memory was
    /// free with 86% of swap in use.
    ///
    /// `report`, and an EMPTY repair map. The difference from disk matters:
    /// the disk default names `build_caches` because a tagged cache directory
    /// is regenerable by the tool that wrote it, and no memory repair is
    /// reversible in that sense — restarting a unit interrupts whatever it
    /// was doing, and terminating a session process loses unsaved work. So
    /// the undeclared host becomes VISIBLE and stays untouched, and arming a
    /// repair is an explicit registry declaration naming that exact repair.
    pub fn reporting_default() -> Self {
        Self {
            mode: "report".to_string(),
            check_interval_seconds: constants::DEFAULT_CHECK_INTERVAL_SECONDS,
            low_free_mb: constants::DEFAULT_LOW_FREE_MB,
            target_free_mb: constants::DEFAULT_TARGET_FREE_MB,
            high_swap_used_pct: constants::DEFAULT_HIGH_SWAP_USED_PCT,
            max_repairs_per_pass: constants::DEFAULT_MAX_REPAIRS_PER_PASS,
            refuse_placement: false,
            max_pass_seconds: None,
            repairs: BTreeMap::new(),
        }
    }

    /// The pass budget in force: the declaration's, or the janitor's own.
    pub fn pass_seconds(&self) -> u64 {
        self.max_pass_seconds
            .and_then(|declared| u64::try_from(declared).ok())
            .unwrap_or(constants::PASS_DEADLINE_SECONDS)
    }

    /// The low watermark in bytes.
    pub fn low_free_bytes(&self) -> i64 {
        self.low_free_mb.saturating_mul(constants::MIB)
    }

    /// The target watermark in bytes.
    pub fn target_free_bytes(&self) -> i64 {
        self.target_free_mb.saturating_mul(constants::MIB)
    }

    /// Whether this policy permits any repair at all. `report` and `off`
    /// never repair whatever the map says.
    pub fn repairs_armed(&self) -> bool {
        self.mode == "enforce" && !self.repairs.is_empty()
    }

    /// The declared policy for one repair name, when the host declared it.
    pub fn repair(&self, name: &str) -> Option<&MemoryRepairPolicy> {
        self.repairs.get(name)
    }
}

/// The declaration this target carries, if any.
///
/// One reader, named once, so that the CLI report, the host reader and the
/// pass cannot disagree about where the declaration lives.
pub fn declared(target: &crate::targets::ComputeTarget) -> Option<MemoryReclaimPolicy> {
    target.memory_reclaim.clone()
}

/// The registry key the declaration is written under.
pub const REGISTRY_KEY: &str = "memory_reclaim";
