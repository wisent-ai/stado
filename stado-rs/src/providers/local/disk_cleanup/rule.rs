//! The disk-full rule: the janitor's only instruction.
//!
//! When the volume holding the agent's home is at least [`DISK_FULL_PERCENT`]
//! used, a pass deletes everything the fleet put on the host — every
//! candidate of every cleaner in [`CLEANERS`], with no age, count, byte or
//! scan limit. Below it a pass deletes nothing. Nothing about cleanup is
//! declared per host: the registry once carried `disk_cleanup` and
//! `memory_reclaim` blocks with caps, ages, watermarks and repairs nobody had
//! approved, and a cap of 10 000 items per pass left a vault host out of disk
//! while its janitor counted what it was not allowed to delete.
//!
//! What a pass keeps is what is not the fleet's to take: `~/.ssh` and
//! anything outside a cleaner's fleet-owned area (the user's data), a backup
//! twin whose primary cannot be proven identical (it is then the only copy),
//! release versions the registry declares or this host has installed, and
//! the work tree of a job that is still running.
//!
//! The volumes are every one the fleet writes to: the one holding `$HOME`,
//! and the one holding the host's declared work root
//! ([`crate::providers::local::work_base`]) when it declares one. Each is
//! measured with `statvfs`, and the one with less room before the threshold
//! is the reading the rule judges ([`read_fleet_volume`]). On macOS the
//! home's volume is the APFS Data volume, whose usage is the container's,
//! not the sealed system snapshot mounted at `/`.

use std::io;
use std::path::Path;

use serde_json::{json, Value};

use crate::providers::local::disk_cleanup::JanitorError;

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
        root: super::backup_twins::BACKUP_ROOT,
        sweeps: "same-disk replica objects whose primary copy is intact",
    },
    Cleaner {
        name: "build_caches",
        root: "",
        sweeps: "directories carrying a build tool's own CACHEDIR.TAG anywhere under the home",
    },
    Cleaner {
        name: super::chromium_clones::CLEANER,
        root: "",
        sweeps: "the operating system's per-launch code-signing clones",
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
        root: super::backup_twins::PRIMARY_ROOT,
        sweeps: "payload outputs of jobs the queue lists as terminal; receipts and logs stay",
    },
    Cleaner {
        name: super::local_snapshots::CLEANER,
        root: "",
        sweeps: "every local Time Machine snapshot except the operating system's update snapshots",
    },
    Cleaner {
        name: super::object_evidence::CLEANER,
        root: super::object_evidence::ROOT,
        sweeps: "product run evidence in this host's object store",
    },
    Cleaner {
        name: super::release_store::CLEANER,
        root: super::release_store::RELEASES_ROOT,
        sweeps: "published release versions no host declares and this host has not installed",
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

/// The reading the rule judges: the volume holding the home and, when the
/// host declares a work root on another volume, that volume too — job trees
/// and build caches fill it. Of the two, the one with less room before the
/// threshold decides, so a full work-root volume starts a pass even while the
/// home's volume is nearly empty.
pub fn read_fleet_volume(home: &Path) -> Result<VolumeReading, JanitorError> {
    let home_reading = read_volume(home)?;
    let Some(root) = crate::providers::local::work_base::declared() else {
        return Ok(home_reading);
    };
    let root_reading = read_volume(&root)?;
    Ok(
        if root_reading.headroom_bytes() < home_reading.headroom_bytes() {
            root_reading
        } else {
            home_reading
        },
    )
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
