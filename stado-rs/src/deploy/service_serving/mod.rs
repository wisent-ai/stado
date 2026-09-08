//! Is the declared unit the process on its own port?
//!
//! NO Python original. This module exists because of what `service show`
//! answered on 2026-08-30. It reported `com.wisent.always-on.weles` as `runs`
//! while both pids the preceding restart had reported were already gone from
//! `ps` and the unit's stderr ended in `EADDRINUSE 127.0.0.1:58101`. The unit
//! was dead and the control plane called it healthy, which is why nobody
//! noticed for days.
//!
//! The reason is worth stating exactly, because it is a shape this fleet has
//! now met three times. `SHOW_BODY` says `runs` whenever the unit FILE exists:
//! it reads `ProgramArguments` out of the plist and reports what the unit
//! declares. That is a useful answer to a different question. It is a
//! declaration nobody checked against the world — the same defect as a forward
//! marker naming a port nothing served, and as an env key no reader ever read
//! back. [`super::host_inventory`] closed the first by reconciling markers
//! against live listeners and [`super::service_env_file`] closed the second by
//! reading a write back. This closes the third, one level further down: not
//! "is something listening there" but "is the thing listening there the
//! process this unit owns".
//!
//! Three properties are deliberate:
//!
//! 1. **Ownership is decided by launchd label, never by argv.** On
//!    charless-mac-mini the declared `com.wisent.always-on.weles` and the
//!    undeclared `com.wisent.weles-worker` execute the same program with the
//!    same argument vector — the Weles release deployer bootstraps the second
//!    one by design. [`super::service::stado_unit_pids`]-style argv matching
//!    would attribute the surviving process to whichever unit was asked about
//!    and answer `serving` for a unit that is down. So the question asked here
//!    is which launchd job holds the pid, resolved by walking the pid's parent
//!    chain until a pid appears in `launchctl list`.
//! 2. **An owner that cannot be read is [`OWNER_UNKNOWN`], never
//!    "undeclared".** An unprivileged `launchctl list` shows the caller's
//!    per-login domain and not the system domain, so a system LaunchDaemon's
//!    label is genuinely unresolvable over the approved channel. Reporting
//!    that as "no unit owns this port" would turn every working daemon into a
//!    finding.
//! 3. **A check that could not be performed is not a check that passed.** The
//!    verdict [`SERVING_UNKNOWN`] exists and exits non-zero, exactly as
//!    [`super::service_env_file`]'s `listeners_state` does, because "nothing
//!    is listening" and "nobody could look" are opposite findings that look
//!    identical in an empty list.
//!
//! The transport is [`host_channel::run_script`](super::host_channel::run_script) and the listener read is the
//! same `lsof -nP -iTCP:<port> -sTCP:LISTEN` spelling
//! [`super::service::LISTENER_RESET_BODY`] already uses, so this command and
//! every other reader of "what is listening" cannot disagree.

mod model;
mod remote;
mod verdicts;

pub use model::{Holder, PortReport, PortVerdict, ServingReport};
pub use remote::{parse_serving, read_serving, remote_serving_script};
pub use verdicts::{failure, port_verdicts, to_report, verdict};

/// `status` for a report that came back whole.
pub const OK_STATUS: &str = "service_serving";

/// The owning launchd label was resolved from the pid's own parent chain.
pub const OWNER_RESOLVED: &str = "resolved";
/// No label in the readable domain claims this pid or any of its ancestors.
/// A system LaunchDaemon is invisible to an unprivileged `launchctl list`, so
/// this is never reported as "nothing owns it".
pub const OWNER_UNKNOWN: &str = "unknown";

/// The process holding this port belongs to the unit under test.
pub const PORT_SERVED_BY_UNIT: &str = "served_by_unit";
/// Something is listening and a DIFFERENT launchd job owns it. The finding
/// this module was written for.
pub const PORT_SERVED_BY_OTHER: &str = "served_by_other";
/// Nothing is listening on the port, and the socket table was really read.
pub const PORT_DEAD: &str = "dead";
/// Something is listening and whose job it is could not be established.
pub const PORT_OWNER_UNKNOWN: &str = "owner_unknown";
/// The socket table could not be read, so the port was not judged.
pub const PORT_UNKNOWN: &str = "unknown";

/// Every declared port is held by this unit's own process.
pub const SERVING_YES: &str = "serving";
/// At least one declared port is dead or held by another job.
pub const SERVING_NO: &str = "not_serving";
/// The question could not be answered.
pub const SERVING_UNKNOWN: &str = "unknown";

/// Listeners came from `lsof`.
pub const LISTENERS_READ: &str = "read";
/// Neither reader answered.
pub const LISTENERS_FAILED: &str = "failed";

/// How many parent links the owner walk follows before giving up. A launchd
/// job's own pid is the process or a near ancestor of it; eight is far past
/// any real launcher chain and bounds the walk on a hostile process tree.
pub const MAX_OWNER_DEPTH: u32 = 8;

/// The cap on ports one report judges.
pub const MAX_PORTS: usize = 32;
