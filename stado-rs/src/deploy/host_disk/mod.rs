//! The host reader behind `stado space report TARGET`: current filesystem and
//! memory usage beside the registry cleanup policy and janitor state.
//!
//! NO Python original: item four of `stado.wisent.com/docs/missing-commands`. Shape and
//! rules come from [`crate::deploy::host_state::reboot`] via
//! [`crate::deploy::host_channel`].
//!
//! Three parts, deliberately reported together. "97% full" on its own does
//! not tell an operator whether anything is going to be done about it, and
//! "the janitor last ran at 04:12" on its own does not say whether it
//! helped. The July incident was precisely the pair coming apart: a box at
//! zero free bytes whose cleanup policy looked fine in the registry.
//!
//! No part invents a schema.
//!
//! - Usage comes from `df -Pk /` — the POSIX output format, so the columns
//!   are the same on macOS and Linux, unlike the default macOS layout,
//!   which inserts three inode columns before the mount point.
//! - Policy comes from the registry's own
//!   [`crate::targets::DiskCleanupPolicy`], serialized as it stands.
//! - State comes from the janitor's own state file, named by
//!   [`crate::providers::local::disk_cleanup::state_relative_path`] and
//!   written by that module's `write_state`. The `last pass`, `freed
//!   bytes` and `next scheduled pass` this command reports are all derived
//!   from that document; nothing here re-implements the janitor's
//!   bookkeeping.
//! - Local APFS snapshots come from `tmutil listlocalsnapshots /`, and they
//!   are here because NOTHING in this product can reclaim them and their
//!   blocks are already inside the `used` figure above. On
//!   `control-host` on 2026-08-18 the janitor's cleaners and the declared
//!   space-reclamation filesystem stages between them accounted for every
//!   consumer an operator could act on, and three OS-update snapshots sat
//!   outside all of it — the kind of thing that holds tens of GiB and turns
//!   "the product says the disk is accounted for" into a false statement.
//!   Reported, never touched. macOS publishes no size for a snapshot:
//!   `tmutil`, `diskutil apfs listSnapshots` and `diskutil info` all name
//!   them and none of them measures them (checked on macOS 26.5 on both this
//!   control plane's host and the mini), so the count and the host's own
//!   names are reported and no byte figure is invented from them.
//!
//! Like [`crate::deploy::host_recovery`]'s script, the remote program is
//! written as an escaped string: `\\t` / `\\n` are the literal backslash
//! sequences the remote `printf` expands.

use chrono::{DateTime, TimeDelta};
use serde_json::{json, Map, Value};

use super::host_channel;
use super::{shlex_quote, DeployError, Runner};
use crate::providers::local::disk_cleanup;
use crate::targets::ComputeTarget;

mod memory;
mod reading;
mod report;
mod script;

pub use memory::*;
pub use reading::*;
pub use report::*;
pub use script::{remote_script, remote_script_for};

/// `status` for a clean read.
pub const OK_STATUS: &str = "ok";

/// Substitution point for the janitor's state path in [`REMOTE_SCRIPT`].
/// The value is a crate constant, never registry or operator data, and it
/// is shell-quoted before it is spliced.
const STATE_PATH_MARK: &str = "@STATE_PATH@";

/// Substitution point for the janitor's lock path in [`REMOTE_SCRIPT`], on
/// the same terms: a crate constant, shell-quoted before it is spliced.
const LOCK_PATH_MARK: &str = "@LOCK_PATH@";

/// Substitution point for the memory pass's own state path, on the same
/// terms as [`STATE_PATH_MARK`]: a crate constant, shell-quoted before it is
/// spliced. `targets[].memory_reclaim` has a janitor state file exactly as
/// `targets[].disk_cleanup` does, and this report reads both.
const MEMORY_STATE_PATH_MARK: &str = "@MEMORY_STATE_PATH@";

/// What the caller of [`remote_script_for`] is going to read.
///
/// The script is the definition of every measurement here, so a caller that
/// consumes fewer fields must not get a second implementation of the ones it
/// shares: each section below is one constant, and every scope splices the
/// same constants. Two scopes therefore cannot drift on a field they both
/// keep, because there is only ever one text producing it.
///
/// This exists because the cost is not evenly spread. [`INVENTORY_SECTION`]
/// walks the managed home and selected system roots deeply enough to attribute
/// disk pressure; the depth caps the OUTPUT, never the traversal, so it walks the whole selected
/// tree. Measured on `lukasz-macbook` on 2026-09-02: the three fields
/// `host gates` reads take 0.8s together, while the full script had not
/// finished after 180s and burned `user 7m27s` of CPU, so
/// `stado host gates lukasz-macbook` died on the two-minute
/// [`host_channel::remote_timeout`] having computed nothing an operator
/// could read — `disk_cleanup_stalled` and `cleanup_success_age_seconds`
/// were unobtainable on the machine the command was running on. The work
/// was performed for a consumer that does not exist: `host gates` never
/// reads `inventory`, `clone_summaries` or `lock_holders`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskScope {
    /// Every field. `space report`'s [`to_report`] reads all eight, so its
    /// script's cost is the cost of what it prints.
    Full,
    /// `usage`, `state` and `snapshots` only: exactly the three fields
    /// `deploy::host_gates::assemble` reads. The omitted sections are
    /// independent commands, so the kept fields are produced by the same
    /// text, in the same order, as under [`DiskScope::Full`].
    GateInputs,
    /// A current free-space reading, independent of janitor and snapshot reads.
    UsageOnly,
    /// Janitor state, memory and snapshots, without repeating the filesystem
    /// measurement.
    StateOnly,
}
