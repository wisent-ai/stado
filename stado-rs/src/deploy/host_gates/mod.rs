//! `stado host gates HOST` — the one payload that answers "why is this host
//! claiming nothing".
//!
//! NO Python original. The incident it exists for: the Mac mini's data volume
//! sat at roughly 2 GiB free against a 55 GiB registry policy. Its queue agent
//! computes [`disk_cleanup::disk_pressure_unresolved`] every tick, publishes
//! true — `accepting_jobs: false`, no new job, deliberately
//! ([`crate::providers::local::agent`]). So the host claimed nothing for
//! hours, every release build queued behind it, and no command in this CLI
//! said any of it: the former disk report printed free bytes and policy but never
//! the admission verdict, `registry doctor` listed the host as broadcasting
//! normally, and the one fact that mattered — the agent had stopped claiming,
//! on purpose, for a reason it was republishing every tick — existed only
//! inside `capacity/<consumer>.json`, which nothing read.
//!
//! Four sources, joined here and re-derived nowhere:
//!
//! - the host's own capacity publication (`capacity/<consumer>.json`), whose
//!   `diag` words are reported VERBATIM. A blocker an operator reads here has
//!   to be greppable in the agent that published it, otherwise the CLI has
//!   invented a second vocabulary for the same condition;
//! - the registry target and its [`crate::targets::DiskCleanupPolicy`]
//!   serialized as they stand;
//! - `df -Pk /` and the janitor's own state file, read with the exact sections
//!   [`crate::deploy::host_disk`] sends, so `host gates` and `space report`
//!   cannot disagree about how much space this host has;
//! - the host's own effective `wc_storage_backend`, read with the exact script
//!   `stado host config-show` sends, and classified by
//!   [`crate::capabilities::storage_reach`]. The fourth source exists because
//!   of a second incident on the same machine: its agent unit was re-declared
//!   with a `STADO_CONFIG` that set the backend to `local`, so the agent
//!   published its capacity into an on-disk store on that one box and read a
//!   stale registry back out of it. Everything above kept reporting normally —
//!   the agent was running, it was publishing, its numbers were internally
//!   consistent — and the only true statement was that no host but that one
//!   could address a single object it wrote. Seventy-four jobs waited days.
//!
//! Read-only, and safe against a live production host: one ssh read of one
//! `df` and one `cat`, one ssh read of `stado config show`, plus one object
//! read. Nothing restarts, nothing cycles, nothing is deleted. The write
//! half — actually getting the space back — is
//! [`crate::deploy::host_reclaim`].
//!
//! The four sources sit in `read`, the vocabulary they are reported in in
//! `words`, the shape they are carried in in `gates`, and the join and its
//! two published documents in `verdict`.
//!
//! [`disk_cleanup::disk_pressure_unresolved`]: crate::providers::local::disk_cleanup::disk_pressure_unresolved

mod gates;
mod read;
mod verdict;
mod words;

pub use gates::{HostGates, WaitingJob};
pub(crate) use read::observe;
pub use read::{read_host_gates, DiagnosticRead, ReadState};
pub use verdict::{assemble, gates_section, to_report};
pub use words::{
    AGENT_DECLARED_NOT_LOADED, AGENT_STORE_DEVICE_ONLY, AGENT_STORE_UNKNOWN,
    AGENT_STORE_UNREADABLE, CAPACITY_PUBLICATION_STALE, CLEANUP_IN_PROGRESS,
    DISK_CLEANUP_LOCK_HELD, DISK_CLEANUP_POLICY_UNKNOWN, DISK_CLEANUP_STALLED,
    DISK_PRESSURE_ACTIVE, DISK_PRESSURE_UNRESOLVED, LOCAL_SNAPSHOTS_UNRECLAIMABLE,
    NO_CAPACITY_PUBLICATION, PINNED_ONLY, QUEUE_PAUSED, RELEASE_SCRATCH_SHORT,
};

pub const HOST_DIAGNOSTIC_INCOMPLETE: &str = "host_diagnostic_incomplete";

pub(super) use read::resolves_to;
