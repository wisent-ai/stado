//! Start, restart, reload, stop, retire and probe — including the one system
//! LaunchDaemon repair that needs no privilege and the privileged path behind
//! it.

mod daemon;
mod restart;
mod stop;

// `privileged` exposes only the sudo path `restart` calls, which reaches it
// directly as `super::privileged`; it re-exports nothing.
mod privileged;

pub use daemon::*;
pub use restart::*;
pub use stop::*;

/// `launchctl bootout`'s exit status when the job is not loaded: 3 (`ESRCH`,
/// "No such process") or 113 ("Could not find specified service"). A job that
/// is already gone is the state a stop asks for, so these are not failures;
/// the decision reads the status the program returned, not its sentence.
const LAUNCHD_JOB_ABSENT: [i32; 2] = [3, 113];

fn launchd_job_absent(code: i32) -> bool {
    LAUNCHD_JOB_ABSENT.contains(&code)
}
