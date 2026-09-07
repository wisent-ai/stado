//! Every number the memory-reclaim pass is built on, in one place.
//!
//! The disk janitor keeps its bounds beside the code that uses them; this
//! module keeps them together instead, because each one below is a judgement
//! about the 2026-09-06 charless-mac-mini incident rather than a unit
//! conversion, and a reader checking whether the watermark would have caught
//! that host should not have to open six files to find out.

/// One mebibyte. The declaration states memory in MiB because a host's free
/// memory is a three-to-four-digit number there, and in GiB the same
/// declaration would need fractions to express the watermark that matters.
pub const MIB: i64 = 1024 * 1024;

/// Percent, as the whole the swap ratio is taken against.
pub const PERCENT: i64 = 100;

/// The memory pass's state-file schema version.
///
/// Separate from the disk janitor's `STATE_VERSION` and in a separate file:
/// the two passes share a lock directory and nothing else, and a reader that
/// required one version to read the other would make a binary upgrade on one
/// side stop the other side's reporting.
pub const STATE_VERSION: i64 = 1;

/// The state directory, shared with the disk janitor (`~/.cache/wisent-compute`).
pub const STATE_DIR_PARTS: [&str; 2] = [".cache", "wisent-compute"];

/// The memory pass's own state file.
pub const STATE_NAME: &str = "memory-reclaim-state.json";

/// The memory pass's own exclusive run lock.
///
/// Its own, not the disk janitor's: a memory pass that waited behind a
/// 13-minute `build_caches` walk would be exactly as late as the walk, and
/// the incident this exists for is measured in the minutes before a process
/// cannot allocate.
pub const LOCK_NAME: &str = "memory-reclaim.lock";

/// Seconds one pass may spend before it stops, when the host declares no
/// `max_pass_seconds`.
///
/// The pass reads two kernel counters and, at most, restarts one declared
/// unit. Ten seconds is generous for that and short enough that the queue
/// agent's janitor task returns well inside one poll interval.
pub const PASS_DEADLINE_SECONDS: u64 = 10;

/// Reporting-default check interval, in seconds.
///
/// Five minutes, which is the fleet's own silence threshold
/// (`STADO_SILENCE_THRESHOLD_SECONDS`, 300): a host is called silent after
/// three missed one-minute beacons, and memory pressure that has held for
/// longer than that is not a spike.
pub const DEFAULT_CHECK_INTERVAL_SECONDS: i64 = 300;

/// Reporting-default low watermark, in MiB.
///
/// charless-mac-mini was holding roughly 1.3 GB free — about 1240 MiB — when
/// its pre-check runner died with `Failed to create CoreCLR, HRESULT:
/// 0x8007000C` and exit 137. 2048 MiB is above that reading and below the
/// idle free memory of every other host in this fleet, so an undeclared host
/// reports the incident state and reports nothing on a healthy one.
pub const DEFAULT_LOW_FREE_MB: i64 = 2048;

/// Reporting-default target watermark, in MiB.
///
/// Twice the low watermark, for the same reason the disk policy's
/// `target_free_gb` is twice its `low_free_gb`: a pass that stopped at the
/// watermark it was called at would be called again on the next interval.
pub const DEFAULT_TARGET_FREE_MB: i64 = 4096;

/// Reporting-default swap watermark, in percent of the swap file in use.
///
/// The mini was at 4.3 of 5 GB — 86% — with 797k pages in the compressor and
/// 12.2M swapouts, while its free-memory figure alone still read as a
/// machine that was merely busy. Swap utilisation is the second watermark
/// because it is the one that was unambiguous.
pub const DEFAULT_HIGH_SWAP_USED_PCT: i64 = 80;

/// Reporting-default per-pass repair budget.
///
/// One. A pass that restarted several units at once would remove the
/// evidence of which one was holding the memory, and the next pass is one
/// interval away.
pub const DEFAULT_MAX_REPAIRS_PER_PASS: i64 = 1;

/// Smallest low watermark a declaration may carry, in MiB.
pub const MIN_LOW_FREE_MB: i64 = 64;

/// Smallest and largest check interval a declaration may carry, in seconds.
/// The same bounds the disk policy validates against, so one host cannot
/// declare two janitors on incomparable cadences.
pub const MIN_CHECK_INTERVAL_SECONDS: i64 = 60;
/// Largest declared check interval, in seconds (one day).
pub const MAX_CHECK_INTERVAL_SECONDS: i64 = 86400;

/// Largest per-pass repair budget a declaration may carry.
pub const MAX_REPAIRS_CEILING: i64 = 8;

/// Bounds on a declared `max_pass_seconds`.
pub const MIN_PASS_SECONDS: i64 = 1;
/// Largest declared pass budget, in seconds.
pub const MAX_PASS_SECONDS: i64 = 600;
