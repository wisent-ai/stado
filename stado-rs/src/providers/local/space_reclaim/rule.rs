//! The disk-full rule: the janitor's only instruction.
//!
//! When the volume the fleet writes to is at least [`DISK_FULL_PERCENT`]
//! used, a pass deletes everything the fleet put on the host — every
//! candidate of every cleaner in [`CLEANERS`], with no age, count, byte or
//! scan limit. Below it a pass deletes nothing. Nothing about cleanup is
//! declared per host: the fleet policy alone supplies the threshold and
//! the cleaner set, rather than per-target ages, caps or watermarks.
//!
//! What a pass keeps is what is not the fleet's to take: `~/.ssh` and
//! anything outside a cleaner's fleet-owned area (the user's data), a backup
//! twin whose primary cannot be proven identical (it is then the only copy),
//! release versions the registry declares or this host has installed, and
//! the work tree of a job that is still running.
//!
//! The volume is the one the fleet writes to: the one holding the host's
//! declared work root ([`crate::providers::local::work_base`]) when it
//! declares one, else the one holding `$HOME`. It is measured with
//! `statvfs` ([`read_fleet_volume`]). A host whose work root sits on a data
//! disk is never called full because its system disk is: every fleet writer
//! puts its trees on the work root, and what earlier releases left under the
//! home is taken on every pass as nothing reuses it. On macOS the home's
//! volume is the APFS Data volume, whose usage is the container's, not the
//! sealed system snapshot mounted at `/`.

use std::io;
use std::path::Path;

use serde_json::{json, Value};

use crate::providers::local::space_reclaim::JanitorError;

/// The operator's threshold, in his words verbatim: "jezeli dysk jest 80%
/// zapelniony, kasujesz z niego wszystko poza podstowowym ssh i danymi
/// uzytkownikow" — at this many percent used, delete everything the fleet
/// put on the host.
pub const DISK_FULL_PERCENT: u8 = 80;

/// One cleaner the rule runs, and the area it takes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cleaner {
    /// The name its report is filed under.
    pub name: &'static str,
    /// Home-relative area it sweeps. Empty when the area is resolved on the
    /// host itself (the whole home for build caches, the macOS per-user
    /// temporary container for Chromium clones, the harness log directories,
    /// Time Machine's local snapshots).
    pub root: &'static str,
    /// What it takes, in one clause.
    pub sweeps: &'static str,
}

/// Every cleaner, in the order a report lists them.
pub const CLEANERS: &[Cleaner] = &[
    Cleaner {
        name: super::agent_logs::CLEANER,
        root: "",
        sweeps: "logs the coding-agent harnesses keep under this account (.omp/logs, .claude/debug, .codex/log, .factory/logs, .kimi-code/logs)",
    },
    Cleaner {
        name: super::backup_twins::CLEANER,
        root: crate::config_file::LOCAL_BACKUP_ROOT,
        sweeps: "same-disk replica objects whose primary copy is intact",
    },
    Cleaner {
        name: "build_caches",
        root: "",
        sweeps: "directories carrying a build tool's own CACHEDIR.TAG anywhere under the home",
    },
    Cleaner {
        name: super::build_caches::CARGO_BUILD_DIRS,
        root: ".cargo/build",
        sweeps: "Cargo's per-workspace build directories ($CARGO_HOME/build/<xx>/<hash>), tagged or not, each whole unless a build holds its lock",
    },
    Cleaner {
        name: super::chromium_clones::CLEANER,
        root: "",
        sweeps: "the operating system's per-launch code-signing clones",
    },
    Cleaner {
        name: super::compiler_cache::CLEANER,
        root: "",
        sweeps: "the compiler cache's store (~/Library/Caches/kache, ~/.cache/kache), while no job runs",
    },
    Cleaner {
        name: super::docker::CLEANER,
        root: "",
        sweeps: "Docker's images, build cache, stopped containers and unused volumes, through the daemon, while no job runs",
    },
    Cleaner {
        name: super::delivered_releases::CLEANER,
        root: super::delivered_releases::DELIVERED_ROOT,
        sweeps: "delivered release copies no installed coordinate or binary names, keeping each product's newest",
    },
    Cleaner {
        name: super::bin_backups::CLEANER,
        root: super::bin_backups::BIN_ROOT,
        sweeps: "backup copies installs leave beside each binary, keeping the newest of each and the .previous rollback target",
    },
    Cleaner {
        name: super::source_snapshots::CLEANER,
        root: ".stado/local-storage/ecosystem/sources",
        sweeps: "build source snapshots no catalog entry, live job or unfinished run names, keeping each product's newest",
    },
    Cleaner {
        name: "huggingface_cache",
        root: ".cache/huggingface/hub",
        sweeps: "model blobs the hub can fetch again",
    },
    Cleaner {
        name: super::queue_workdirs::CLEANER,
        root: ".stado/work/jobs",
        sweeps: "work trees of jobs the queue reports terminal",
    },
    Cleaner {
        name: super::job_outputs::CLEANER,
        root: crate::config_file::LOCAL_STORAGE_ROOT,
        sweeps: "payload outputs of jobs the queue lists as terminal; receipts and logs stay",
    },
    Cleaner {
        name: super::local_snapshots::CLEANER,
        root: "",
        sweeps: "every local Time Machine snapshot of the volume the home lives on, except the operating system's update snapshots",
    },
    Cleaner {
        name: super::object_evidence::CLEANER,
        root: super::object_evidence::ROOT,
        sweeps: "product run evidence in this host's object store",
    },
    Cleaner {
        name: super::registered_scratch::CLEANER,
        root: "",
        sweeps: "trees a job, an agent or a Stado verb registered with `stado space scratch register`, once the session that owns each has ended",
    },
    Cleaner {
        name: super::package_caches::CLEANER,
        root: "",
        sweeps: "the package managers' download caches (Cargo git checkouts, npm, Bun, SwiftPM, pip, uv, Homebrew), largest first, only while agent output alone left the volume at the rule and only until it is under it; a cache a running process holds stays",
    },
    Cleaner {
        name: super::release_store::CLEANER,
        root: super::release_store::RELEASES_ROOT,
        sweeps: "published release versions no host declares and this host has not installed",
    },
    Cleaner {
        name: super::queue_workdirs::session_checkouts::CLEANER,
        root: "",
        sweeps: "git checkouts a fleet session is proven to have cloned (its clone marker, or this host's queue record of the session that cloned it), under ~/Documents/CodingProjects on a dedicated host on every pass and under <work root>/session-checkouts at the rule; each whole once no process works in it and git reports nothing in it that no remote has; every other checkout there stays",
    },
    Cleaner {
        name: super::xcode_derived_data::CLEANER,
        root: super::xcode_derived_data::ROOT,
        sweeps: "Xcode's per-project build directories (DerivedData/<project>-<hash>), each whole unless a running process has a file in it open or names it; archives and device support are never touched",
    },
    Cleaner {
        name: "weles_recordings",
        root: "weles/recordings",
        sweeps: "Weles run recordings, uploaded or not",
    },
];

/// One reading of a volume's capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeReading {
    pub total_bytes: i64,
    pub free_bytes: i64,
}

impl VolumeReading {
    /// Percent of the volume in use, as `df` computes it from the space an
    /// unprivileged writer may still take.
    pub fn used_percent(&self) -> f64 {
        used_percent(self.total_bytes, self.free_bytes)
    }

    /// Whether the rule deletes on this reading.
    pub fn full(&self) -> bool {
        full(self.total_bytes, self.free_bytes)
    }

    /// Bytes a writer may still add before the rule starts deleting.
    pub fn headroom_bytes(&self) -> i64 {
        headroom_bytes(self.total_bytes, self.free_bytes)
    }
}

/// Percent used from a total and the free bytes an unprivileged writer has.
pub fn used_percent(total_bytes: i64, free_bytes: i64) -> f64 {
    if total_bytes <= 0 {
        return 0.0;
    }
    let free = free_bytes.clamp(0, total_bytes) as f64;
    100.0 * (1.0 - free / total_bytes as f64)
}

/// Whether a volume with this total and free space is at the threshold.
pub fn full(total_bytes: i64, free_bytes: i64) -> bool {
    total_bytes > 0 && used_percent(total_bytes, free_bytes) >= f64::from(DISK_FULL_PERCENT)
}

/// Free bytes below which the volume is at the threshold: the part of the
/// volume the rule keeps free.
pub fn reserve_bytes(total_bytes: i64) -> i64 {
    total_bytes / 100 * i64::from(100 - DISK_FULL_PERCENT)
}

/// Bytes that may still be written before the volume reaches the threshold;
/// zero or negative once it has.
pub fn headroom_bytes(total_bytes: i64, free_bytes: i64) -> i64 {
    free_bytes - reserve_bytes(total_bytes)
}

/// Read the volume holding `path`.
pub fn read_volume(path: &Path) -> Result<VolumeReading, JanitorError> {
    let stat = nix::sys::statvfs::statvfs(path)
        .map_err(|error| JanitorError::from(io::Error::from_raw_os_error(error as i32)))?;
    let unit = stat.fragment_size() as i64;
    Ok(VolumeReading {
        total_bytes: (stat.blocks() as i64).saturating_mul(unit),
        free_bytes: (stat.blocks_available() as i64).saturating_mul(unit),
    })
}

/// The reading the rule judges: the volume holding the host's declared work
/// root, where every fleet writer puts its trees, else the volume holding
/// the home. The home's own volume is not judged on a host that declares a
/// work root elsewhere: a system disk filled by what is not the fleet's must
/// not stop the fleet from claiming work on a data disk with room.
pub fn read_fleet_volume(home: &Path) -> Result<VolumeReading, JanitorError> {
    match crate::providers::local::work_base::declared() {
        Some(root) => read_volume(&root),
        None => read_volume(home),
    }
}

/// Whether the home's own volume is at the threshold on a host whose work
/// root is declared elsewhere. It is not the rule's verdict and stops no
/// claim, but what the fleet rebuilds or refetches under the home (Cargo's
/// build directories, tagged build caches) is taken while it holds, so a
/// system disk the rule no longer judges is still kept clean of the fleet's
/// caches. A home volume that cannot be read takes nothing this way.
pub fn home_volume_full(home: &Path) -> bool {
    crate::providers::local::work_base::declared().is_some()
        && read_volume(home).is_ok_and(|reading| reading.full())
}

/// The `rule` object every report carries: the threshold, the reading it
/// was judged on, and the verdict.
pub fn rule_json(reading: Option<VolumeReading>) -> Value {
    json!({
        "full_percent": DISK_FULL_PERCENT,
        "used_percent": reading.map(|reading| (reading.used_percent() * 10.0).round() / 10.0),
        "triggered": reading.is_some_and(|reading| reading.full()),
    })
}
