//! Classify a host's local disaster-recovery replica against the store it is
//! supposed to mirror, object by object, before anything is deleted.
//!
//! Written for charless-mac-mini on 2026-08-30, where the replica had become
//! the largest single consumer on the one machine whose disk was blocking a
//! stalled queue:
//! 48.5 GiB of `~/.stado/local-backup` against a 32.7 GiB
//! primary. It got there because replication crossed two addressings — the
//! object API answers in bare ecosystem keys, a directory in
//! namespace-qualified store paths — so every pass wrote its objects at names
//! nothing resolves, and [`crate::queue::copy::prune_backup_extras`] only ever
//! swept the canonical prefixes, leaving the rest to accumulate for the lifetime
//! of the host.
//!
//! Reclaiming it needs a number nobody had: how much of that replica exists,
//! intact, in the primary. The first attempt at a similar question — the
//! doubly-nested tree in the primary — was assumed to be duplicate data and
//! turned out to be the ONLY copy of 9.58 GiB of trained-model artifacts, which
//! is the whole reason this command reports a classification rather than
//! deleting anything. Nothing here removes a byte.
//!
//! **Everything runs on the host, and no object body crosses the network.** Both
//! stores are directories on that machine — the API's own backing store and the
//! replica beside it — so the comparison is local file work. That matters beyond
//! tidiness: pulling 134 MiB bodies through the control plane's loopback writer
//! is what took that host's release ingress down earlier the same day.
//!
//! Classification, per file in the replica:
//!
//! - **`twin`** — the primary holds the same address with the same size AND the
//!   same SHA-256. Only these are safe to drop.
//! - **`differs`** — the primary holds that address with different content. Data,
//!   and possibly the newer of the two. Kept.
//! - **`absent`** — the primary does not hold that address at all. This is the
//!   sole-copy case, and on this host it is the expected verdict for everything
//!   the mis-addressed replication wrote. Kept.
//!
//! Hashing is the expensive half, so it runs only where a size match already
//! makes a twin possible; `absent` and a size mismatch are decided without
//! reading a byte.
//!
//! The address mapping is the same rule the copier now refuses to cross. A
//! replica path already under `ecosystem/` is a qualified store path and maps
//! straight through; a bare path is what a cross-addressed pass wrote, and its
//! primary address is that path inside the configured namespace.
//!
//! The pass is split by what it reads and what it writes down: `plan` holds
//! the request and the program text it produces, `remote_program` the fixed
//! program itself, `report` the reading, and `parse` the fold from the
//! program's marker lines into it.

use std::time::Duration;

use crate::targets::ComputeTarget;

use super::host_channel;
use super::{DeployError, Runner};

mod parse;
mod plan;
mod remote_program;
mod report;

pub use parse::*;
pub use plan::*;
pub use report::*;

/// Proven present in the primary with identical bytes. Only these are safe to
/// drop.
pub const TWIN: &str = "twin";
/// The primary has this address with different content. Data; kept.
pub const DIFFERS: &str = "differs";
/// The primary does not have this address at all. The sole-copy case; kept.
pub const ABSENT: &str = "absent";
/// The primary has this address at the same size, but the pass ran out of its
/// hashing budget before proving the bytes match.
///
/// Reported as its own class rather than folded into [`TWIN`], because the one
/// thing this command exists to prevent is treating an unproven twin as
/// reclaimable. A size match is not identity.
pub const SAME_SIZE_UNPROVEN: &str = "same_size_unproven";

/// Classify `host`'s replica against its primary store, and — when the plan
/// says so — delete the twins the same pass just proved.
///
/// The proof and the deletion are one pass on purpose. An audit written to a
/// file and a deletion run against it later is how a safety net becomes data
/// loss: the addresses move, the primary changes, and the recorded verdict
/// stops describing the disk. Nothing in this module can act on a verdict it
/// did not compute in the same run.
pub async fn audit_host(
    host: &str,
    plan: &AuditPlan,
    runner: &Runner,
) -> Result<(ComputeTarget, BackupAudit), DeployError> {
    let target = host_channel::canonical_target(host).await?;
    let script = remote_script(plan);
    let output = if plan.reclaim {
        host_channel::run_script_with_timeout(
            &target,
            &script,
            Duration::from_secs(RECLAIM_TIMEOUT_SECONDS),
            runner,
        )
        .await?
    } else {
        host_channel::run_script(&target, &script, runner).await?
    };
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the host did not classify its replica",
        )));
    }
    let audit = parse_output(&output.stdout, &target.name);
    Ok((target, audit))
}
