//! Backend-to-backend copy of the whole queue store.
//!
//! NO Python original: this module is new in the Rust runtime, so there is
//! no parity note to make. In-store migrations handle schema changes, while
//! this command is the sole supported cross-backend transfer path. It builds
//! both explicit endpoints inside their provider adapters and never relies on
//! an operator cloud CLI or ambient ADC copier.
//!
//! Both ends are built from explicit locators through [`Endpoint::build`],
//! never through [`crate::queue::JobStorage`]: the facade resolves its
//! backend from the ambient `WC_STORAGE_BACKEND`, so source and destination
//! could never differ inside one process. The credentials path is
//! unchanged — [`Endpoint::build`] calls the same backend constructors the
//! facade calls.
//!
//! What this guarantees:
//!
//! - [`copy`] itself never deletes at either end. The coordinator-only
//!   [`replicate_configured_backup`] reconciliation prunes stale objects from
//!   the designated backup after a fully clean copy; it never deletes source.
//! - **Metadata travels with the body.** `JobStorage::write_job` stamps
//!   `gpu_mem_gb` / `priority` / `gpu_type` on every queue blob in a
//!   separate `set_metadata` round trip, and the oldest-first pass inside
//!   `queue::listing::list_claimable` prefilters on the stamped `gpu_mem_gb`
//!   before it downloads a job document at all — the pass that runs whenever
//!   the destination's marker index has not yet been swept end-to-end, which
//!   is precisely the state a fresh copy leaves it in.
//!   A body-only copy would silently degrade
//!   every scheduler tick into downloading the whole queue, so each object
//!   gets its source metadata re-applied at the destination — and the
//!   result is verified, because
//!   `<AzureBlobBackend as BlobBackend>::set_metadata` logs and SWALLOWS
//!   every write failure (Python parity) and therefore proves nothing by
//!   returning `Ok`.
//! - **Resumable and idempotent.** A [`SENTINEL_PATH`] marker in the
//!   DESTINATION store records the last cleanly finished prefix plus the
//!   running counts (same shape as the `queue_priority/.migration.json`
//!   sentinel in `queue/migrations.rs`), and every object whose body already
//!   matches at the destination is skipped.
//!
//! Cost note: [`BlobBackend`] exposes no body digest — [`BlobInfo`] carries
//! only name, timestamp and metadata — so deciding whether an existing
//! destination object matches reads the body at both ends. Re-runs are
//! therefore cheap in WRITES, not in READS.
//!
//! The components mirror the seams this module already carried: [`endpoint`]
//! builds and describes one end of the copy, [`report`] holds the knobs and
//! the per-object, per-prefix and whole-run results, [`objects`] copies one
//! prefix object by object and verifies what landed, and [`run`] drives a
//! whole run, its resume sentinel and the disaster-recovery replication.
//! Every name this module used to declare is re-exported here, so
//! `crate::queue::copy::<item>` resolves exactly as before.
//!
//! [`BlobBackend`]: super::BlobBackend
//! [`BlobInfo`]: super::BlobInfo

mod endpoint;
mod objects;
mod report;
mod run;

pub use endpoint::Endpoint;
pub use report::{
    CopyOptions, CopyPlan, CopyReport, ObjectReport, Outcome, PrefixPlan, PrefixReport,
};
pub use run::{copy, plan, replicate_configured_backup};

/// Every prefix that makes up the queue store, in copy order.
///
/// Hardcoded rather than discovered, because a bare root listing of a live
/// bucket is both enormous and racy, and because two of these entries are
/// invisible to the obvious enumeration:
///
/// - `cancelled/` is NOT one of the prefixes `JobStorage::list_all_jobs`
///   walks (it covers queue/running/completed/uploaded/failed only), so a
///   copy driven by the job listing would silently drop every cancelled
///   job. That is precisely why it is spelled out here.
/// - `queue_priority/` carries the `.migration.json` sentinel of
///   `queue::migrations` alongside the priority markers. It is copied with
///   the rest of the prefix so the destination inherits the completed
///   backfill instead of re-running it.
/// - `ecosystem/` is the provider-neutral product object data plane. Queue
///   migration and disaster-recovery backup must carry it with scheduler
///   state or migrated jobs would point at objects left in the failed store.
///
/// `registry.json` is a root object, not a directory; it is listed as a
/// prefix because every [`BlobBackend`] listing is a plain string-prefix
/// match, so the full object name selects exactly that one object.
///
/// `job-transitions/` holds the durable transition documents
/// (`queue::storage::TRANSITION_PREFIX`) that make a claim, a completion and
/// a cancellation survive a crash between two writes. It arrived on
/// 2026-09-01 without a line here or in any host's object policy, and the
/// object API on `charless-mac-mini` answered every agent claim with 401
/// until 2026-09-03 while the agent restarted after each one. This list is
/// what `config::queue_prefixes_missing` holds every `probierz` object policy
/// to, so a prefix the binary uses is granted before the binary is delivered.
///
/// [`BlobBackend`]: super::BlobBackend
pub const CANONICAL_PREFIXES: &[&str] = &[
    "queue/",
    "job-transitions/",
    "running/",
    "completed/",
    "uploaded/",
    "failed/",
    "cancelled/",
    "queue_priority/",
    "scripts/",
    "status/",
    "capacity/",
    "provider-leases/",
    "runs/",
    "fixed/",
    "failed_again/",
    "schedules/",
    "cancellations/",
    "machine_requests/",
    "machine_inputs/",
    "config/",
    "state/",
    "operations/",
    "failure_fixes/",
    "coverage/",
    "host_health/",
    "billing_health/",
    "hf_rate/",
    "artifacts/",
    "ecosystem/",
    "registry.json",
];

/// Resume sentinel, written to the DESTINATION store. Mirrors
/// `queue::migrations::SENTINEL_PATH`; deliberately outside
/// [`CANONICAL_PREFIXES`] so a later copy never treats it as queue state.
pub const SENTINEL_PATH: &str = "storage_copy/.copy.json";

/// Default copy fan-out. Reuses the crate's existing bulk-download budget
/// (`queue::migrations::BULK_WORKERS`) rather than inventing a second
/// concurrency style.
pub const DEFAULT_CONCURRENCY: usize = super::migrations::BULK_WORKERS;
