//! Fleet-wide queue pause switch — maintenance mode and the supported
//! pre-migration drain.
//!
//! NO Python original: the Python CLI has no `queue` group and nothing in
//! that tree reads a pause flag. This is item twenty of
//! `docs/missing-commands.md` ("maintenance mode: stop/start dispatching
//! without cancelling queued jobs"), and the prerequisite every storage
//! migration already assumed existed. `deploy/migrate_to_stado.sh` refuses
//! to execute unless the operator sets `CONFIRM_FLEET_DRAINED=yes` — an
//! honour-system claim with, until now, no mechanism behind it — and
//! `cli/storage.rs::print_split_brain_warning` spells out what
//! copying a live queue costs: a job claimed from the old store, written
//! to the new one, and reaped from neither.
//!
//! # What a pause STOPS
//!
//! * Dispatch. `scheduler::scheduler::schedule_queued_jobs` returns zero
//!   dispatched before it reads quota or creates anything, so no instance
//!   is provisioned and no new cloud spend starts.
//! * New claims. The local-agent poll loop
//!   (`providers::local::agent::run_agent`) skips its queue scan, so no
//!   agent moves a job out of `queue/` into `running/`.
//!
//! # What a pause does NOT stop
//!
//! * Jobs already in `running/`. They keep their slot, keep heartbeating,
//!   upload their output, and land in `completed/` or `failed/` exactly as
//!   usual. That asymmetry is the whole point: nothing new enters
//!   `running/`, so what is in it drains out and
//!   [`is_drained`] eventually holds. A pause that also froze running work
//!   would never terminate.
//! * Queued work. Pausing is NOT cancelling. Every job keeps its place,
//!   its priority and its assignment in `queue/`, and dispatch picks up
//!   where it left off on `stado queue resume`. Removing a job is still
//!   `stado cancel`'s job.
//! * Everything else the fleet does. The monitor still reaps dead
//!   instances and requeues orphans, agents still broadcast capacity and
//!   still run the disk janitor, and a due cron schedule still submits —
//!   its job simply waits in `queue/` with the rest.
//!
//! # Storage shape
//!
//! One small JSON document at [`CONTROL_BLOB`], under the `config/` prefix
//! `queue::copy::CANONICAL_PREFIXES` already carries, so a
//! backend-to-backend migration takes the pause state with it: a
//! half-migrated fleet stays paused on BOTH stores instead of waking up
//! mid-copy.
//!
//! Writes are compare-and-swap over a versioned read — the pattern of
//! `schedules.rs::claim_due`, including its create-if-absent fallback —
//! because this blob is overwritten in place rather than written once.
//! Two operators flipping the switch in the same second must not silently
//! lose one intent. Reads pin the download to the current generation for
//! the reason `schedules.rs::read_fresh_text` documents: an in-place
//! overwrite can otherwise read back edge-cached, and an agent reading a
//! stale copy would keep claiming after the pause landed.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::models::isoformat_utc;

use super::storage::JobStorage;
use super::StorageError;

/// The pause switch document.
pub const CONTROL_BLOB: &str = "config/queue_control.json";

/// Prefix holding work that has not been claimed yet.
pub const QUEUED_PREFIX: &str = "queue";

/// Prefix holding claimed work. A drain is over when this is empty.
pub const RUNNING_PREFIX: &str = "running";

/// The prefixes `stado queue status` reports and `drain` watches, in
/// display order: what is waiting, and what still has to finish.
pub const WATCHED_PREFIXES: &[&str] = &[QUEUED_PREFIX, RUNNING_PREFIX];

/// `JobStorage::list_paths`'s "no oldest-first bound" sentinel — the
/// parameter caps the listing only when it is greater than zero.
const UNBOUNDED_LISTING: usize = usize::MIN;

/// The pause switch. Absent blob == every field defaulted == not paused,
/// so a fleet that has never been paused needs no bootstrap write.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueControl {
    /// True while dispatch and new claims are suspended.
    pub paused: bool,
    /// Operator note carried into every agent and scheduler log line that
    /// reports the pause, so the fleet says WHY it is idle.
    pub reason: String,
    /// RFC-3339 timestamp of the flip that produced this state.
    pub since: String,
    /// Who flipped it — the hostname the CLI ran on unless the caller
    /// passed something more specific.
    pub by: String,
}

impl QueueControl {
    /// Pretty JSON, `ensure_ascii`-escaped like every other document this
    /// crate writes (`schedules.rs::Schedule::to_json`).
    pub fn to_json(&self) -> String {
        let pretty =
            serde_json::to_string_pretty(self).expect("QueueControl serialization is infallible");
        crate::models::ensure_ascii(&pretty)
    }

    /// One-line "why is the fleet idle" for the scheduler and agent logs
    /// that report a refusal. Unset fields are dropped rather than
    /// printed as blanks, so an operator who paused without `--reason`
    /// still gets a useful line.
    pub fn pause_summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.reason.is_empty() {
            parts.push(format!("reason: {}", self.reason));
        }
        if !self.since.is_empty() {
            parts.push(format!("since {}", self.since));
        }
        if !self.by.is_empty() {
            parts.push(format!("by {}", self.by));
        }
        if parts.is_empty() {
            return "no reason recorded".to_string();
        }
        parts.join(", ")
    }
}

/// Current pause state; an absent or empty blob is the default.
///
/// Callers on a loop MUST call this every iteration rather than caching
/// it — an operator's `stado queue resume` has to reach a running agent
/// without restarting it.
pub async fn read(store: &JobStorage) -> Result<QueueControl, StorageError> {
    let Some(versioned) = store.read_text_versioned(CONTROL_BLOB).await? else {
        return Ok(QueueControl::default());
    };
    if versioned.content.trim().is_empty() {
        return Ok(QueueControl::default());
    }
    Ok(serde_json::from_str(&versioned.content)?)
}

/// Flip the switch and return the state that was persisted.
///
/// Compare-and-swap over the versioned read: an absent blob is created
/// create-only, and a blob deleted between the read and the swap falls
/// back to that same create-only path (`schedules.rs::claim_due`). A lost
/// race is reported, never retried silently — two operators disagreeing
/// about maintenance mode is exactly the case where the loser has to see
/// the winner's write before deciding again.
///
/// An empty `by` resolves to this machine's hostname
/// (`watchdog::hostname`).
pub async fn set_paused(
    store: &JobStorage,
    paused: bool,
    reason: &str,
    by: &str,
) -> Result<QueueControl, StorageError> {
    let state = QueueControl {
        paused,
        reason: reason.to_string(),
        since: isoformat_utc(Utc::now()),
        by: if by.is_empty() {
            crate::watchdog::hostname()
        } else {
            by.to_string()
        },
    };
    let body = state.to_json();
    let won = match store.read_text_versioned(CONTROL_BLOB).await? {
        None => store.create_text_if_absent(CONTROL_BLOB, &body).await?,
        Some(versioned) => {
            match store
                .compare_and_swap_text(CONTROL_BLOB, &versioned.version, &body)
                .await
            {
                Ok(_) => true,
                Err(StorageError::StorageConflict(_)) => false,
                Err(StorageError::NotFound(_)) => {
                    store.create_text_if_absent(CONTROL_BLOB, &body).await?
                }
                Err(exc) => return Err(exc),
            }
        }
    };
    if !won {
        return Err(StorageError::StorageConflict(format!(
            "{CONTROL_BLOB} changed while this flip was in flight — another operator ran \
             `stado queue pause` or `stado queue resume` at the same moment. Check \
             `stado queue status` and retry."
        )));
    }
    Ok(state)
}

/// Job blob names directly under `<prefix>/`.
///
/// Names only: a count and an emptiness check must not download 14k job
/// bodies, which is what `JobStorage::list_jobs` would do.
async fn job_blobs(store: &JobStorage, prefix: &str) -> Result<Vec<String>, StorageError> {
    let paths = store
        .list_paths(&format!("{prefix}/"), UNBOUNDED_LISTING)
        .await?;
    Ok(paths
        .into_iter()
        .filter(|name| name.ends_with(".json"))
        .collect())
}

/// How many jobs sit under `<prefix>/`.
pub async fn job_count(store: &JobStorage, prefix: &str) -> Result<usize, StorageError> {
    Ok(job_blobs(store, prefix).await?.len())
}

/// True when `running/` holds nothing — the condition `queue drain --wait`
/// blocks on, and the claim `deploy/migrate_to_stado.sh` asks the operator
/// to make with `CONFIRM_FLEET_DRAINED=yes`.
pub async fn is_drained(store: &JobStorage) -> Result<bool, StorageError> {
    Ok(job_blobs(store, RUNNING_PREFIX).await?.is_empty())
}

/// Default deadline for `stado queue drain --wait`, in seconds.
///
/// Derived from `config::HEARTBEAT_STALE_MINUTES`, the window after which
/// the monitor declares a running job's heartbeat dead and requeues it. A
/// drain that has waited that long has given every slot a full staleness
/// window to either finish or be reaped, so anything still in `running/`
/// past it is genuinely long work — an operator decision, not a longer
/// sleep.
pub fn default_drain_timeout_s() -> u64 {
    chrono::Duration::minutes(config::HEARTBEAT_STALE_MINUTES)
        .num_seconds()
        .unsigned_abs()
}
