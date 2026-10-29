//! The delivery routine's pass, fired by every `stado serve` of a host that
//! declares a delivery plan (`stado ci plan set`): the coordinator's tick and
//! the release reconciliation interval both ask it on each of their ticks, so
//! a host that reconciles releases without coordinating delivers too. A pass
//! of `stado ci run` starts only once the plan's declared cadence (`--every`)
//! has passed since the last pass was attempted, one the operator's rulings
//! refused included, and not while a pass holds the routine's lock, so no
//! host builds or asks more often than the plan states. The
//! pass runs as a process of its own beside the tick, so a pass that builds
//! for an hour holds no tick back. A host with no plan fires nothing; a plan
//! that cannot be read, or that states no cadence, is said in the tick's log
//! with its cause, once per process, and fires nothing.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};

use crate::cli::ci::delivery::{plan, state};
use crate::cli::CmdError;

/// What the tick's log names the pass by while it waits for it.
const PASS: &str = "the delivery pass `stado ci run`";

/// Whether this process already said why it fires no pass.
static SAID: AtomicBool = AtomicBool::new(false);

/// When the next pass of `plan` is due: its cadence after the start of the
/// pass that began `last`, now when no pass ever began; refused, naming how
/// to declare one, when the plan states no cadence.
pub(crate) fn next_due(
    plan: &plan::Plan,
    last: Option<DateTime<Utc>>,
) -> Result<DateTime<Utc>, CmdError> {
    let every = plan.every_seconds.ok_or_else(|| {
        CmdError::refused(format!(
            "the delivery plan declared at {} states no cadence, so no pass is fired for it: \
             declare it again with `stado ci plan set ... --every SECONDS`",
            plan.declared_at
        ))
    })?;
    let Some(started) = last else {
        return Ok(Utc::now());
    };
    let seconds = i64::try_from(every.get()).map_err(|error| {
        CmdError::usage(format!("--every {every} is more seconds than a clock holds: {error}"))
    })?;
    chrono::TimeDelta::try_seconds(seconds)
        .and_then(|step| started.checked_add_signed(step))
        .ok_or_else(|| {
            CmdError::usage(format!(
                "{every} seconds after {started} is past the clock's last moment"
            ))
        })
}

/// Why no pass is due now, or `None` when one is: no plan, a pass holding
/// the lock, or the cadence not yet passed are silent reasons; a plan or
/// state that cannot be read, and a plan with no cadence, are said.
fn not_due() -> Option<Option<String>> {
    let plan = match plan::read() {
        Ok(Some(plan)) => plan,
        Ok(None) => return Some(None),
        Err(error) => {
            return Some(Some(format!(
                "delivery plan unreadable, so no delivery pass is fired: {error}"
            )))
        }
    };
    match state::lock_holder() {
        Ok(None) => {}
        Ok(Some(_)) => return Some(None),
        Err(error) => {
            return Some(Some(format!(
                "the delivery pass lock cannot be read, so no pass is fired: {error}"
            )))
        }
    }
    let last = match state::last_started() {
        Ok(last) => last,
        Err(error) => {
            return Some(Some(format!(
                "the delivery state cannot be read, so no pass is fired: {error}"
            )))
        }
    };
    match next_due(&plan, last) {
        Ok(due) if due <= Utc::now() => None,
        Ok(_) => Some(None),
        Err(refusal) => Some(Some(refusal.to_string())),
    }
}

/// Fire one pass of the delivery routine beside the coordinator's tick when
/// one is due, logging in the tick's log.
pub(super) fn fire() {
    fire_due(super::log);
}

/// Fire one pass of the delivery routine when one is due, saying through
/// `say` why none was fired (once per process) and how a fired pass ended.
pub(crate) fn fire_due(say: fn(&str)) {
    match not_due() {
        None => {}
        Some(None) => return,
        Some(Some(why)) => {
            if !SAID.swap(true, Ordering::Relaxed) {
                say(&why);
            }
            return;
        }
    }
    SAID.store(false, Ordering::Relaxed);
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(error) => {
            say(&format!(
                "this Stado's executable cannot be named, so no delivery pass is fired: {error}"
            ));
            return;
        }
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(["ci", "run", "--json"])
        .stdin(std::process::Stdio::null());
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            say(&format!("{PASS} did not start: {error}"));
            return;
        }
    };
    say(&format!("{PASS} started: its cadence has passed"));
    tokio::spawn(async move {
        let ended = crate::wait::until(
            crate::wait::Kind::Process,
            PASS,
            "this host",
            child.wait_with_output(),
        )
        .await;
        match ended {
            Ok(output) if output.status.success() => say(&format!(
                "{PASS} ended: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            )),
            Ok(output) => say(&format!(
                "{PASS} failed ({}): {} {}",
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => say(&format!("{PASS} could not be awaited: {error}")),
        }
    });
}
