//! The host reader behind `stado space report TARGET`: current filesystem and
//! memory usage beside the disk-full rule's verdict and the janitor's state.
//!
//! NO Python original: item four of `stado.wisent.com/docs/missing-commands`. Shape and
//! rules come from [`crate::deploy::host_state::reboot`] via
//! [`crate::deploy::host_channel`].
//!
//! Report usage, the rule and janitor state together. Low free space does not
//! show whether cleanup is active, and a completed pass does not prove that
//! it brought the volume under the threshold.
//!
//! No part invents a schema.
//!
//! - Usage comes from `df -Pk /` — the POSIX output format, so the columns
//!   are the same on macOS and Linux, unlike the default macOS layout,
//!   which inserts three inode columns before the mount point.
//! - The rule's verdict is [`crate::providers::local::disk_cleanup::rule`]
//!   applied to that usage.
//! - State comes from the janitor's own state file, named by
//!   [`crate::providers::local::disk_cleanup::state_relative_path`] and
//!   written by that module's `write_state`. The `last pass` and `freed
//!   bytes` this command reports are derived from that document; nothing
//!   here re-implements the janitor's bookkeeping.
//! - Local APFS snapshots come from `tmutil listlocalsnapshots /`, and they
//!   are here because NOTHING in this product can reclaim them and their
//!   blocks are already inside the `used` figure above. The janitor's
//!   cleaners and the declared space-reclamation filesystem stages between
//!   them account for every consumer an operator can act on, and OS-update
//!   snapshots sit outside all of it — the kind of thing that holds tens of
//!   GiB and turns "the product says the disk is accounted for" into a
//!   false statement.
//!   Reported, never touched. macOS publishes no size for a snapshot:
//!   `tmutil`, `diskutil apfs listSnapshots` and `diskutil info` all name
//!   them and none of them measures them (checked on current macOS on
//!   managed hosts), so the count and the host's own
//!   names are reported and no byte figure is invented from them.
//!
//! Like [`crate::deploy::host_recovery`]'s script, the remote program is
//! written as an escaped string: `\\t` / `\\n` are the literal backslash
//! sequences the remote `printf` expands.

use chrono::DateTime;
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
/// tree. The three fields `host gates` reads take under a second together,
/// while the full script can run for minutes and burn several CPU-minutes,
/// so `stado host gates <host>` would die on the two-minute
/// [`host_channel::remote_timeout`] having computed nothing an operator
/// could read — `disk_cleanup_stalled` and `cleanup_success_age_seconds`
/// unobtainable on the machine the command runs on. That work would be
/// performed for a consumer that does not exist: `host gates` never reads
/// `inventory`, `clone_summaries` or `lock_holders`.
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
