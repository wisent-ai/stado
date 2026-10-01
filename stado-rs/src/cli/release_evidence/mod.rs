//! `stado release logs` and `stado release doctor` — the candidate's own
//! account of why a rollout stopped, and one verdict over every fact that
//! decides whether it can ever finish.
//!
//! NO Python original. Both exist because a candidate that dies quickly
//! leaves only the outside view in what the fleet publishes: the rollout
//! state says `candidate did not become ready within 90s: pid <pid> is gone`
//! and nothing else. The candidate's own stderr — the process explaining its
//! own exit — sits in `~/.stado/logs/<product>-<version>.err` on the host,
//! and without these commands nothing in the CLI reads that file.
//!
//! So `release logs` fetches exactly that file, and `release doctor` answers
//! the question the operator actually asks before opening it — "will this
//! rollout ever land, and if not, what is holding it" — by joining the four
//! facts that are each individually visible and otherwise never assembled:
//! desired versus observed release, the candidate's liveness and health, the
//! quarantine map (a desired digest sitting in it is a rollout that will
//! never retry), and the host's claiming gates (a host can stop claiming on
//! `disk_pressure_unresolved` with nothing else in the CLI saying so).
//!
//! Both are strictly read-only and safe against a live production host:
//! they read files and ask one loopback readiness URL for its status. They
//! start nothing, stop nothing, and write nothing — neither takes a
//! `--reason`, because neither changes any state to audit.
//!
//! The remote transport is not this module's: file reads go through
//! [`crate::cli::release_quarantine`]'s readers, which ride the one
//! registry ssh channel ([`crate::deploy::host_channel`]). The only remote
//! program here is the candidate probe below, and it is a compile-time
//! script with three quoted bindings.

use crate::cli::CmdError;

mod constants;
mod doctor;
mod logs;
mod quarantine;

use doctor::doctor;
use logs::logs;

pub use constants::*;
pub use doctor::ReleaseDoctorArgs;
pub use logs::ReleaseLogsArgs;

pub async fn dispatch_logs(args: &ReleaseLogsArgs) -> Result<(), CmdError> {
    logs(args).await
}

pub async fn dispatch_doctor(args: &ReleaseDoctorArgs) -> Result<(), CmdError> {
    doctor(args).await
}
