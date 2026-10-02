//! `stado host gates HOST` — the one payload that answers "why is this host
//! claiming nothing".
//!
//! Admission depends on the agent's observed state, not merely on a running
//! process or a recent heartbeat. For example, unresolved disk pressure can
//! make the agent publish `accepting_jobs: false` and deliberately claim no
//! work. This command exposes that decision together with its inputs.
//!
//! Diagnostic sources, joined here and re-derived nowhere:
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
//!   [`crate::capabilities::storage_reach`]. Device-local capacity and registry
//!   writes can succeed while remaining inaccessible to the fleet; process
//!   health and locally consistent state do not prove shared storage reach.
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
    DISK_ATTACHED_UNMOUNTED, DISK_CLEANUP_LOCK_HELD, DISK_CLEANUP_POLICY_UNKNOWN,
    DISK_CLEANUP_STALLED, DISK_PRESSURE_ACTIVE, DISK_PRESSURE_UNRESOLVED,
    LOCAL_SNAPSHOTS_UNRECLAIMABLE, NO_CAPACITY_PUBLICATION, PINNED_ONLY, QUEUE_PAUSED,
    RELEASE_SCRATCH_SHORT,
};

pub const HOST_DIAGNOSTIC_INCOMPLETE: &str = "host_diagnostic_incomplete";

pub(super) use read::resolves_to;
