//! `stado service unit show` — ask the host init system what it holds under
//! one named unit identity.
//!
//! Enumeration cannot find a loaded unit whose file was deleted, and a unit
//! file cannot say which environment or executable image launchd already
//! loaded. On launchd this reports fixed state fields, the five non-secret
//! storage-routing variables required by recovery, and the running process
//! start, executable path, and digest. It never emits the rest of launchd's
//! environment. A bounded exact-label event tail supplies recent spawn/exit
//! context. Systemd reports the matching fixed properties.
//!
//! It signals nothing, loads nothing and stops nothing. `service bootout` or
//! `service remove` are the commands that act; this one only answers.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use super::service::{validate_unit_id, BootoutScope};
use super::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

mod parse;

mod script;
mod state;

use script::label_print_script;

pub use parse::parse_label_print;
pub use state::{LabelReadFailure, LabelState};

/// The name the launchd event read is admitted and recorded under in the
/// beacon's `scans`.
pub const EVENT_SCAN: &str = "unit_events";

/// What a host's previous event read of one unit left behind: the unit's
/// key and the events read then. A later read whose unit shows the same
/// key carries these instead of reading the log again, and a read the host
/// is too loaded for carries them too.
#[derive(Clone, Debug)]
struct Carried {
    key: String,
    events: Vec<String>,
    status: Option<String>,
    /// When this process last looked at the unit, read or carried: a unit
    /// whose key changed since changed after this moment, so its events are
    /// all younger than it.
    checked_at: std::time::Instant,
}

/// Per (host, label), for the life of this process: the beacon's role is
/// the one reader that reads every unit once a minute.
static CARRIED: LazyLock<Mutex<HashMap<(String, String), Carried>>> = LazyLock::new(Mutex::default);

/// The window a one-off inspection (`service list`, a lifecycle check) reads
/// launchd's event log over: the hour `log show --last` was always asked
/// for, which a reader looking at one unit once affords.
const ONE_OFF_WINDOW: &str = "1h";

/// How the event log is read for one unit, in the script's own words, and
/// over which `log show --last` window.
enum EventRead {
    Read(String),
    Carry,
    Same(String, String),
    Skip,
}

impl EventRead {
    fn word(&self) -> String {
        match self {
            Self::Read(_) => "read".to_string(),
            Self::Carry => "carry".to_string(),
            Self::Same(key, _) => format!("same:{key}"),
            Self::Skip => "skip".to_string(),
        }
    }

    fn window(&self) -> &str {
        match self {
            Self::Read(window) | Self::Same(_, window) => window,
            Self::Carry | Self::Skip => ONE_OFF_WINDOW,
        }
    }
}

/// A `log show --last` window covering `elapsed`: whole minutes, rounded up,
/// never less than one.
fn window_since(elapsed: std::time::Duration) -> String {
    format!("{}m", elapsed.as_secs().div_ceil(60).max(1))
}

/// The window of a cadenced reader's first read of a unit: its declared
/// cadence (`WC_HEALTH_INTERVAL_SECONDS`, the beacon's), so a host process
/// that starts does not read an hour of launchd's log for every unit before
/// its first beacon; the one-off hour when no cadence is declared.
fn first_window() -> String {
    match std::env::var("WC_HEALTH_INTERVAL_SECONDS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
    {
        Some(seconds) => window_since(std::time::Duration::from_secs(seconds)),
        None => ONE_OFF_WINDOW.to_string(),
    }
}

/// Ask one host what it holds under one label, refusing an inconclusive
/// negative observation so internal lifecycle callers cannot treat a failed
/// domain read as proof that the unit is absent.
pub async fn print_label(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<LabelState, DeployError> {
    let state = inspect_label(target, label, scope, runner).await?;
    if !state.loaded() {
        if let Some(detail) = state.read_failure_detail() {
            return Err(DeployError::unreachable(format!(
                "{}: could not determine whether {label} is loaded: {detail}",
                state.host
            )));
        }
    }
    Ok(state)
}

/// Ask one host what it holds under one label while retaining unavailable
/// domain evidence for diagnostic callers.
///
/// Signals nothing. This reads only the host's init-system state, and reads
/// launchd's event log for the unit every time, over [`ONE_OFF_WINDOW`].
pub async fn inspect_label(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<LabelState, DeployError> {
    let read = EventRead::Read(ONE_OFF_WINDOW.to_string());
    inspect_label_with(target, label, scope, runner, read).await
}

/// The unit's state alone, for a reader that decides by it and shows no
/// events: launchd's event log, which `log show` reads for minutes on a busy
/// host, is not read, and `event_read_status` says so.
pub async fn inspect_label_state(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<LabelState, DeployError> {
    inspect_label_with(target, label, scope, runner, EventRead::Skip).await
}

/// The same read for a reader that repeats it on a cadence: launchd's event
/// log, the costly half of the read, is read only when the unit is not as
/// it was at this process's previous read of it (its pid, run count or
/// state changed) and the host is under its scan load ceiling; otherwise
/// the previous read's events are carried, and `event_read_status` says so.
/// A read covers only the time since this process last looked at the unit
/// (the first, its declared cadence): a change since then happened in that
/// window, and an hour of a busy Mac's log took many minutes per unit, which
/// held the beacon that many minutes.
pub async fn inspect_label_carrying(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<LabelState, DeployError> {
    let key = (target.name.clone(), label.to_string());
    // One get or insert per read: a read that panicked while recording left
    // the map whole, so it is taken back instead of refusing every later
    // inspection of every unit.
    let previous = crate::primitives::lock::held(&CARRIED).get(&key).cloned();
    let checked_at = std::time::Instant::now();
    // Nothing to carry: the first read of this unit reads the log whatever
    // the load, since an unread unit has no events at all, so the ceiling
    // is not asked and no skip is recorded for a read that happened.
    let mode = match &previous {
        None => EventRead::Read(first_window()),
        Some(carried) => {
            let admitted = crate::deploy::host_scan::admit(EVENT_SCAN)
                .map_err(|error| {
                    DeployError(format!("the event read could not be admitted: {error}"))
                })?
                .admitted();
            if admitted {
                EventRead::Same(
                    carried.key.clone(),
                    window_since(carried.checked_at.elapsed()),
                )
            } else {
                EventRead::Carry
            }
        }
    };
    let compared = matches!(mode, EventRead::Same(..));
    let mut state = inspect_label_with(target, label, scope, runner, mode).await?;
    let carried = state
        .event_read_status
        .as_deref()
        .is_some_and(|status| status.starts_with("carried:"));
    if carried {
        if let Some(mut previous) = previous {
            state.recent_events = previous.events.clone();
            if let Some(status) = previous.status.clone() {
                let carried_status = state.event_read_status.take();
                state.event_read_status = Some(match carried_status {
                    Some(carried_status) => format!("{carried_status}; that read: {status}"),
                    None => status,
                });
            }
            // Only a read that compared the unit's key saw it unchanged; a
            // load-carried one compared nothing, so the window stays open
            // from the last look that did.
            if compared {
                previous.checked_at = checked_at;
                crate::primitives::lock::held(&CARRIED).insert(key, previous);
            }
        }
    } else if let Some(key_now) = state.event_key.clone() {
        crate::primitives::lock::held(&CARRIED).insert(
            key,
            Carried {
                key: key_now,
                events: state.recent_events.clone(),
                status: state.event_read_status.clone(),
                checked_at,
            },
        );
    }
    Ok(state)
}

async fn inspect_label_with(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
    events: EventRead,
) -> Result<LabelState, DeployError> {
    validate_unit_id(label)?;
    let predicate = format!(
        "process == \"launchd\" AND eventMessage CONTAINS \"{}\"",
        label.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let script = label_print_script(
        &shlex_quote(label),
        &shlex_quote(&predicate),
        &scope.to_string(),
        &shlex_quote(&events.word()),
        &shlex_quote(events.window()),
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.succeeded() {
        return Err(DeployError::unreachable(host_channel::last_error_line(
            &output,
            "the label print did not complete",
        )));
    }
    Ok(parse_label_print(&target.name, label, &output.stdout))
}
