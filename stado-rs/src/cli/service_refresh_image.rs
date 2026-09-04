//! `stado service refresh-image NAME` — put one unit back on the file it
//! declares, and prove it landed.
//!
//! `registry doctor` grew a `stale-unit-image` row that ends "Restarting the
//! unit is what puts it on the installed file, and nothing does that on its
//! own". That sentence told an operator to perform an action the product did
//! not offer as a checked operation. This is the verb behind it.
//!
//! Three properties, each one measured rather than assumed:
//!
//! - **It refuses a unit that is not stale**, and the refusal names the
//!   identity it found. A command that restarts whatever it is pointed at is a
//!   restart button, and this fleet has already turned a degraded host into a
//!   down one with one of those.
//! - **It re-reads the identity afterwards.** On 2026-09-03 pid 49727 —
//!   `com.wisent.compute.agent.lukasz-macbook` — respawned under `KeepAlive`
//!   straight back onto the same unlinked inode 182274754 it had just left.
//!   launchd re-execs the PATH, and the path was never the problem; a
//!   remediation that reported success on the strength of having issued a
//!   restart would have been a second silence exactly where the first one was.
//!   So a restart that did not change the image is a failure with a non-zero
//!   exit, not a caveat in a success message.
//! - **One unit per invocation.** No `--all`, no glob, no sweep. Three stale
//!   units on a host is three deliberate commands, because the blast radius of
//!   a sweep across a fleet agent, a janitor and a stream writer is the whole
//!   host.
//!
//! The release agent's scheduled revisit pass now reuses this module's
//! [`refresh_outcome`] and the same `observe_unit_images` predicate, so a
//! manual refresh and an automatic one reach their verdict by one route. The
//! two callers keep different blast radii and that is deliberate: this
//! command is one named unit per invocation, typed by an operator who has
//! read the row, and the scheduled caller may touch only the exact launchd
//! labels the registry's top-level `release_unit_image_revisit` block
//! authorises for that host — a key no registry carries today — one of them
//! per tick. Neither widens the other.
//!
//! The predicate is not reimplemented here. `deploy::service::
//! observe_unit_images` is the one pass `registry doctor` reads, so a unit this
//! command calls stale and a unit the doctor reports are the same set by
//! construction rather than by two implementations agreeing today.

use std::time::Duration;

use serde_json::{json, Value};

use crate::deploy::service::{self, ImageIdentity, ImageState, UnitImageObservation};

use super::{registry, CmdError};

/// How long to wait for launchd to bring the unit back after `kickstart -k`.
///
/// The unit is stopped and started again, so the window has to cover a process
/// exit and a fresh exec. 30 seconds against a measured restart-to-first-work
/// latency of 55 seconds for the janitor — which included a whole cleanup pass
/// — is enough to see the process appear, which is all that is being waited
/// for; the work it then does is not this command's claim.
const RESTART_WINDOW: Duration = Duration::from_secs(30);

/// How often to re-read while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// `stado service refresh-image NAME [--json]`.
pub async fn refresh_image(name: &str, json_output: bool) -> Result<(), CmdError> {
    let registry = registry::read_registry().await?;
    let hostname = crate::providers::vast::system_hostname();
    let local = registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "no registry target names this machine ({hostname}), and which image a process \
                 is executing is readable only on the machine holding that process; run this \
                 there"
            ))
        })?;
    let host = local.name.clone();

    let before = observe(local, &host, name)?;
    let (running, installed) = actionable(&before)?;

    let service =
        service::kickstart_local_unit(&before.unit, &before.unit_path).map_err(|reason| {
            CmdError::click(format!("{} was not restarted: {reason}", before.unit))
        })?;

    let after = settle(local, &host, name, before.pid).await;
    emit(&before, after.as_ref(), &service, json_output);
    verdict(&before, after.as_ref(), &running, &installed)
}

/// This unit's observation, or the reason there is none.
fn observe(
    target: &crate::targets::ComputeTarget,
    host: &str,
    name: &str,
) -> Result<UnitImageObservation, CmdError> {
    let now = chrono::Utc::now().timestamp();
    service::observe_unit_images(target, Some(host), now)
        .into_iter()
        .find(|row| row.unit == name)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{host} holds no launchd unit named {name} with a live process. Either no unit \
                 file in launchd's three directories declares that label, or the unit is loaded \
                 and not running — a job that is not running holds no image, so there is nothing \
                 to refresh. `stado registry doctor` lists the units this machine was measured on"
            ))
        })
}

/// The identities to act on, or a refusal.
///
/// A unit that is not stale is refused with the identity that was read, so the
/// refusal is checkable rather than a bare "no".
fn actionable(row: &UnitImageObservation) -> Result<(ImageIdentity, ImageIdentity), CmdError> {
    match &row.state {
        Some(
            ImageState::Unlinked { running, installed }
            | ImageState::Replaced { running, installed },
        ) => Ok((running.clone(), installed.clone())),
        Some(ImageState::Unread { subject, reason }) => Err(CmdError::click(format!(
            "{} was not restarted, because whether it is stale is unknown: {subject} could not be \
             read — {reason}. An unread state is not a reason to act any more than it is a reason \
             to pass",
            row.unit
        ))),
        None => Err(CmdError::click(refusal(row))),
    }
}

/// Why a unit that is not stale is left alone, naming what was found.
fn refusal(row: &UnitImageObservation) -> String {
    let pid = row
        .pid
        .map_or_else(|| "its process".to_string(), |pid| format!("pid {pid}"));
    match (&row.running, &row.installed) {
        (Some(running), Some(installed)) if running.is_same_file(installed) => format!(
            "{} is not stale and was not restarted: {pid} is executing {}, which IS the file its \
             ProgramArguments name at {}. Restarting it would be an outage with nothing to fix",
            row.unit,
            running.describe(),
            installed.path
        ),
        (Some(running), Some(installed)) => format!(
            "{} was not restarted: {pid} is executing {} and its declared file at {} is {}, but \
             that file was written less than {}s ago. A replacement inside that window is an \
             installer mid-flight, not a unit left behind — re-run once it has settled",
            row.unit,
            running.describe(),
            installed.path,
            installed.describe(),
            service::IMAGE_SETTLE_SECONDS
        ),
        _ => format!(
            "{} was not restarted: neither identity was read, so there is no evidence it is stale",
            row.unit
        ),
    }
}

/// Wait for the unit to come back, and read its image again.
///
/// `None` when nothing was executing that unit's argument vector by the end of
/// the window — which is itself a failure, and is reported as one.
///
/// Crate-visible and `async` because the release agent's scheduled revisit
/// pass waits for exactly this and must reach the same answer: a second
/// settle loop would be a second definition of "the unit came back". `async`
/// rather than blocking so that a tick which is waiting thirty seconds for
/// launchd is not a tick holding a runtime worker for thirty seconds.
pub(crate) async fn settle(
    target: &crate::targets::ComputeTarget,
    host: &str,
    name: &str,
    was: Option<u32>,
) -> Option<UnitImageObservation> {
    let deadline = std::time::Instant::now() + RESTART_WINDOW;
    let mut last = None;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        let now = chrono::Utc::now().timestamp();
        let found = service::observe_unit_images(target, Some(host), now)
            .into_iter()
            .find(|row| row.unit == name);
        if let Some(row) = found {
            // A row whose pid is the one that was just kicked is launchd not
            // having got there yet, not an answer. Anything else — a new pid,
            // or a read that failed — is the state to report.
            let settled = row.pid != was;
            last = Some(row);
            if settled {
                return last;
            }
        }
    }
    last
}

fn emit(
    before: &UnitImageObservation,
    after: Option<&UnitImageObservation>,
    service: &str,
    json_output: bool,
) {
    let identity = |image: Option<&ImageIdentity>| {
        image.map_or(Value::Null, |image| {
            json!({
                "path": image.path,
                "device": image.device,
                "inode": image.inode,
                "bytes": image.bytes,
                "links": image.links,
            })
        })
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "unit": before.unit,
                "host": before.host,
                "unit_path": before.unit_path,
                "program": before.program,
                "restarted": service,
                "before": {
                    "pid": before.pid,
                    "running": identity(before.running.as_ref()),
                    "installed": identity(before.installed.as_ref()),
                },
                "after": after.map_or(Value::Null, |row| json!({
                    "pid": row.pid,
                    "running": identity(row.running.as_ref()),
                    "installed": identity(row.installed.as_ref()),
                    "agrees": row.agrees(),
                })),
            }))
            .unwrap_or_default()
        );
        return;
    }
    println!("restarted {service}");
    if let (Some(running), Some(pid)) = (before.running.as_ref(), before.pid) {
        println!("  before  pid {pid} was executing {}", running.describe());
    }
    match after {
        Some(row) => {
            let pid = row
                .pid
                .map_or_else(|| "no pid".to_string(), |pid| format!("pid {pid}"));
            match row.running.as_ref() {
                Some(running) => println!("  after   {pid} is executing {}", running.describe()),
                None => println!("  after   {pid}, and its image could not be read"),
            }
            if let Some(installed) = row.installed.as_ref() {
                println!("  declared {} is {}", installed.path, installed.describe());
            }
        }
        None => println!("  after   nothing is executing that unit's argument vector"),
    }
}

/// What the second read found.
///
/// Separated from the sentence and the exit code so the two branches that
/// cannot be reached without a real launchd restart — the one that worked and
/// the one that did not take effect — are still decided by tested logic rather
/// than by code nothing has ever executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The new process is executing the file the unit declares. The only
    /// success.
    OnDeclaredFile,
    /// Nothing is executing the unit's argument vector any more.
    NotRunning,
    /// A process is there and its identity could not be read.
    Unread,
    /// The new process is on the same image the old one was: launchd re-exec'd
    /// the declared path, and the path was never the problem.
    Unchanged,
    /// On some third file: neither the old image nor the declared one.
    StillWrong,
}

impl RefreshOutcome {
    /// Whether this outcome is the command succeeding.
    pub fn succeeded(self) -> bool {
        matches!(self, Self::OnDeclaredFile)
    }

    /// The one word a report names this outcome by.
    ///
    /// Crate-visible because the release agent's scheduled revisit pass
    /// records and prints these same five results, and a second set of words
    /// for them would be a second vocabulary an operator has to learn to
    /// compare a manual refresh with an automatic one. No caller outside the
    /// crate needs it.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::OnDeclaredFile => "OnDeclaredFile",
            Self::NotRunning => "NotRunning",
            Self::Unread => "Unread",
            Self::Unchanged => "Unchanged",
            Self::StillWrong => "StillWrong",
        }
    }
}

/// Read the outcome out of the post-restart observation.
///
/// `was_running` is the image the unit held BEFORE the restart, which is the
/// only way to tell a restart that did nothing from one that landed somewhere
/// unexpected. Pure, and public so both post-restart branches are exercisable
/// without a launchd unit to point at.
pub fn refresh_outcome(
    was_running: &ImageIdentity,
    after: Option<&UnitImageObservation>,
) -> RefreshOutcome {
    let Some(after) = after else {
        return RefreshOutcome::NotRunning;
    };
    let (Some(running), Some(installed)) = (after.running.as_ref(), after.installed.as_ref())
    else {
        return RefreshOutcome::Unread;
    };
    if running.is_same_file(installed) {
        return RefreshOutcome::OnDeclaredFile;
    }
    if running.is_same_file(was_running) {
        return RefreshOutcome::Unchanged;
    }
    RefreshOutcome::StillWrong
}

/// The exit code and the sentence, decided by what the second read found and
/// never by the fact that a restart was issued.
fn verdict(
    before: &UnitImageObservation,
    after: Option<&UnitImageObservation>,
    was_running: &ImageIdentity,
    installed: &ImageIdentity,
) -> Result<(), CmdError> {
    let outcome = refresh_outcome(was_running, after);
    if outcome.succeeded() {
        return Ok(());
    }
    let unit = &before.unit;
    let running = after
        .and_then(|row| row.running.as_ref())
        .map_or_else(|| "an unread image".to_string(), ImageIdentity::describe);
    let declared = after
        .and_then(|row| row.installed.as_ref())
        .unwrap_or(installed);
    Err(CmdError::click(match outcome {
        RefreshOutcome::OnDeclaredFile => unreachable!("handled above"),
        RefreshOutcome::NotRunning => format!(
            "{unit} was restarted and nothing is executing its argument vector {}s later. The \
             unit is now not running at all, which is worse than the stale image it was on: \
             check `stado service status {unit}`",
            RESTART_WINDOW.as_secs()
        ),
        RefreshOutcome::Unread => format!(
            "{unit} was restarted and the result could not be read, so whether it is on the \
             installed file is unknown. That is not the same as fixed"
        ),
        RefreshOutcome::Unchanged => format!(
            "{unit} was restarted and the restart did not take effect: the new process is \
             executing the same {running} it was on before. launchd re-execs the declared path \
             and the path was never the problem — pid 49727 did exactly this on 2026-09-03. The \
             declared file at {} is {}",
            declared.path,
            declared.describe()
        ),
        RefreshOutcome::StillWrong => format!(
            "{unit} was restarted and is executing {running}, which is still not the file it \
             declares at {} ({}). Something replaced the file again between the restart and this \
             read, or the unit reaches its program through a path that resolves elsewhere",
            declared.path,
            declared.describe()
        ),
    }))
}
