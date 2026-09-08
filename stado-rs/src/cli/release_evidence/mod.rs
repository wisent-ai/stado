//! `stado release logs` and `stado release doctor` — the candidate's own
//! account of why a rollout stopped, and one verdict over every fact that
//! decides whether it can ever finish.
//!
//! NO Python original. Both exist because of one diagnosis that the shipped
//! commands could not finish. A brama candidate on `control-host` died
//! in under ninety seconds, and every fact the fleet published about it was
//! the outside view: the rollout state said
//! `candidate did not become ready within 90s: pid 46748 is gone` and
//! nothing else. The candidate's own stderr — the process explaining its own
//! exit — sat unread in `/Users/charles/.stado/logs/brama-0.2.27.err` on the
//! host, because nothing in the CLI reads that file. The operator guessed,
//! then read it by hand over ssh.
//!
//! So `release logs` fetches exactly that file, and `release doctor` answers
//! the question the operator actually asked before opening it — "will this
//! rollout ever land, and if not, what is holding it" — by joining the four
//! facts that were each individually visible and never once assembled:
//! desired versus observed release, the candidate's liveness and health, the
//! quarantine map (a desired digest sitting in it is a rollout that will
//! never retry), and the host's claiming gates (the same Mac mini stopped
//! claiming for hours on `disk_pressure_unresolved` with nothing in the CLI
//! saying so).
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

pub(crate) use quarantine::record_cause;

pub async fn dispatch_logs(args: &ReleaseLogsArgs) -> Result<(), CmdError> {
    logs(args).await
}

pub async fn dispatch_doctor(args: &ReleaseDoctorArgs) -> Result<(), CmdError> {
    doctor(args).await
}
