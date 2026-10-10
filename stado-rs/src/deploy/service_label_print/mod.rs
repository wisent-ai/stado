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
}

/// Per (host, label), for the life of this process: the beacon's role is
/// the one reader that reads every unit once a minute.
static CARRIED: LazyLock<Mutex<HashMap<(String, String), Carried>>> = LazyLock::new(Mutex::default);

/// How the event log is read for one unit, in the script's own words.
enum EventRead {
    Read,
    Carry,
    Same(String),
}

impl EventRead {
    fn word(&self) -> String {
        match self {
            Self::Read => "read".to_string(),
            Self::Carry => "carry".to_string(),
            Self::Same(key) => format!("same:{key}"),
        }
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
/// launchd's event log for the unit every time.
pub async fn inspect_label(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<LabelState, DeployError> {
    inspect_label_with(target, label, scope, runner, EventRead::Read).await
}

/// The same read for a reader that repeats it on a cadence: launchd's event
/// log, the costly half of the read, is read only when the unit is not as
/// it was at this process's previous read of it (its pid, run count or
/// state changed) and the host is under its scan load ceiling; otherwise
/// the previous read's events are carried, and `event_read_status` says so.
pub async fn inspect_label_carrying(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<LabelState, DeployError> {
    let key = (target.name.clone(), label.to_string());
    let previous = CARRIED
        .lock()
        .map_err(|poisoned| {
            DeployError(format!(
                "the carried unit events are poisoned: a read ended while recording ({poisoned})"
            ))
        })?
        .get(&key)
        .cloned();
    // Nothing to carry: the first read of this unit reads the log whatever
    // the load, since an unread unit has no events at all, so the ceiling
    // is not asked and no skip is recorded for a read that happened.
    let mode = match &previous {
        None => EventRead::Read,
        Some(carried) => {
            let admitted = crate::deploy::host_scan::admit(EVENT_SCAN)
                .map_err(|error| {
                    DeployError(format!("the event read could not be admitted: {error}"))
                })?
                .admitted();
            if admitted {
                EventRead::Same(carried.key.clone())
            } else {
                EventRead::Carry
            }
        }
    };
    let mut state = inspect_label_with(target, label, scope, runner, mode).await?;
    let carried = state
        .event_read_status
        .as_deref()
        .is_some_and(|status| status.starts_with("carried:"));
    if carried {
        if let Some(previous) = previous {
            state.recent_events = previous.events;
            if let Some(status) = previous.status {
                let carried_status = state.event_read_status.take();
                state.event_read_status = Some(match carried_status {
                    Some(carried_status) => format!("{carried_status}; that read: {status}"),
                    None => status,
                });
            }
        }
    } else if let Some(key_now) = state.event_key.clone() {
        CARRIED
            .lock()
            .map_err(|poisoned| {
                DeployError(format!(
                    "the carried unit events are poisoned: a read ended while recording \
                     ({poisoned})"
                ))
            })?
            .insert(
                key,
                Carried {
                    key: key_now,
                    events: state.recent_events.clone(),
                    status: state.event_read_status.clone(),
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
        scope.word(),
        &shlex_quote(&events.word()),
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError::unreachable(host_channel::last_error_line(
            &output,
            "the label print did not complete",
        )));
    }
    Ok(parse_label_print(&target.name, label, &output.stdout))
}
