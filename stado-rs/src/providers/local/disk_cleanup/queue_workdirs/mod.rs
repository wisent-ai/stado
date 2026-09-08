//! Terminal queue-job workdir cleanup.
//!
//! The local agent owns every job tree under
//! `$HOME/.stado/work/jobs/wc-<job_id>`. This owner-visible persistent root is
//! deliberate: a workload's cwd and open output descriptor survive an unlink,
//! but their pathname does not. When job trees lived in `/tmp`, external temp
//! cleanup could therefore remove a running job's diagnostics while the
//! janitor's workload lock was held. Release jobs submitted through an older
//! agent leave a narrow compatibility symlink at `/tmp/wc-<job_id>` so that the
//! old agent can finish heartbeats and artifact upload after relocating the
//! tree; this cleaner removes only such owner-matched symlinks after their jobs
//! are terminal.
//!
//! A job workdir is safe to remove when its job is neither queued nor running.
//! Age is not the gate: build workdirs can fill a host within minutes, while a
//! live job must be retained indefinitely. An unreadable queue store therefore
//! removes nothing rather than guessing. Tree deletion is dir-fd-relative,
//! non-following, same-device and holds the canonical root throughout. The only
//! operation under `/tmp` is unlinking a direct-child symlink whose exact
//! canonical target and terminal job id have both been verified.
//!
//! Layout: `roots` resolves the owned root components and admits one job's
//! canonical tree, which is where every path this cleaner may touch comes
//! from; `inventory` is the read-only first pass that names the bounded
//! candidate population the queue authority is asked about; `reclaim` is the
//! non-following tree deletion, the canonical pass that spends the budget and
//! writes the report, and the legacy `/tmp` bridge that pass ends with. This
//! module owns the cleaner's name, the workdir name prefix and the
//! compatibility root all three share.

mod inventory;
mod reclaim;
mod roots;

pub use inventory::candidate_job_ids;
pub use reclaim::pass::scan_queue_workdirs;
pub use roots::{create_work_dir, work_dir, work_root, work_root_in, WORK_ROOT};

/// The cleaner's registry name, and the key its counts appear under in the
/// janitor's report. Declared here rather than spelled at each use, because
/// [`crate::targets`]'s allowed-cleaner list, the report, and this scan have to
/// name the same cleaner or a policy authorizes a pass that never runs.
pub const CLEANER: &str = "queue_workdirs";

/// Directory name prefix for queue-owned job workdirs.
pub const WORKDIR_PREFIX: &str = "wc-";

/// Compatibility root used only by agents that predate the persistent root.
const LEGACY_WORK_ROOT: &str = "/tmp";
