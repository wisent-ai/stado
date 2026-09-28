//! Handing a listener from a role unit to the role inside the product's one
//! process.
//!
//! The old resolver unit and `stado serve --resolver` bind the same ports.
//! While the old unit holds them the new resolver's bind fails, and the
//! supervisor ends the whole `stado serve` over it, so the replacement comes
//! and goes with a new pid each time. A switched-on flag proves nothing; the
//! proof is the resolver's own published word that its listeners are bound,
//! written after the old unit stepped aside. That word is kept apart from the
//! registry's health: a failed refresh publishes `backing_off` with the
//! listeners still bound, so `serving` alone would miss an acquisition.
//!
//! The host keeps one record per unit in `~/.stado/role-handoffs/<unit>`,
//! keyed by the unit and not by a pid, because the replacement's pid does not
//! survive the handoff it is waiting for: when the handoff started, the
//! autostart scopes it withdrew, and the replacement artefact it was for.
//!
//! - No record, and the replacement runs the role: the unit steps aside
//!   (`handed_over`). Only a caller that can bring the unit back starts one.
//! - The resolver published its listeners bound after the record was written:
//!   the listener is acquired. The record says `acquired` until the old
//!   unit's retirement is confirmed, which is retried every pass, and
//!   `complete` after it.
//! - `complete`: the unit stays retired. Registry health after that is the
//!   resolver's own business: a `backing_off` over a failed refresh keeps its
//!   listeners bound, and bringing the old unit back beside it would only
//!   collide with them.
//! - `handed_over`, and it published anything since without its listeners
//!   (other than `starting`), or the registry holds the replacement stopped:
//!   the listener was never acquired, so the withdrawn scopes are restored and
//!   the record is marked `refused` for that artefact, so the declared unit is
//!   repaired.
//! - `handed_over` with nothing published since, or the replacement between
//!   two restarts: the resolver has not answered; the unit stays out.
//! - `refused`: kept until a different replacement artefact is installed, or
//!   until the replacement serves anyway, which only happens once the old unit
//!   let go of the ports.
//!
//! Every decision follows what the host published; none follows a clock.

use crate::deploy::service::*;

use super::record::{read_published, read_record, write_record};

/// The `readiness` a catalog role unit names when its role shares the old
/// unit's listener and only the resolver's published state proves it.
pub const RESOLVER_STATE: &str = "resolver-state";

/// Where a role unit whose role shares its listener stands on one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// The replacement's resolver acquired the listener: record it and keep
    /// the unit retired.
    Complete,
    /// Acquired earlier: the unit stays retired whatever the resolver says now.
    Retained(String),
    /// Nothing was tried for this replacement: the unit may step aside.
    Start,
    /// Handed over; the resolver has not published since.
    Waiting(String),
    /// Handed over and the role did not take: bring the unit back, and refuse
    /// `artefact`, the replacement build the handoff was for.
    Restore {
        scopes: Vec<String>,
        artefact: String,
        detail: String,
    },
    /// The unit keeps its work: why.
    Kept(String),
}

/// Decide from the replacement's process, whether it runs the role (`None`)
/// or why not, whether the registry holds the replacement `stopped` rather
/// than between two restarts, the host's record and what the resolver
/// published.
pub async fn handoff_standing(
    target: &ComputeTarget,
    process: &RunningProgram,
    not_running: Option<&str>,
    stopped: bool,
    unit: &str,
    runner: &Runner,
) -> Result<Handoff, DeployError> {
    let record = read_record(target, unit, runner).await?;
    let published = read_published(target, &process.declared, runner).await?;
    let current: Option<u32> = process.pid.trim().parse().ok();
    // Before any handoff the old unit may be the one listening, so only the
    // replacement's own pid counts.
    let listening_now = not_running.is_none()
        && published
            .as_ref()
            .is_some_and(|seen| seen.listening && Some(seen.pid) == current);
    // After the old unit stepped aside, anything that bound the ports is the
    // replacement's resolver, even a pid that has restarted since: the
    // listener was acquired, whatever the registry's health says.
    let acquired_since = |since: i64| {
        published
            .as_ref()
            .is_some_and(|seen| seen.listening && seen.written > since)
    };
    let Some(record) = record else {
        return Ok(match not_running {
            Some(reason) => Handoff::Kept(reason.to_string()),
            None if listening_now => Handoff::Complete,
            None => Handoff::Start,
        });
    };
    if record.state == "complete" {
        return Ok(Handoff::Retained(format!(
            "{unit} handed its listener to the resolver of {}",
            record.artefact
        )));
    }
    // Acquired, and the old unit's retirement not yet confirmed: retry it,
    // never undo it, whatever the resolver says now.
    if record.state == "acquired" {
        return Ok(Handoff::Complete);
    }
    if acquired_since(record.since) {
        return Ok(Handoff::Complete);
    }
    if record.state == "refused" {
        return Ok(
            if not_running.is_none() && process.resolved != record.artefact {
                Handoff::Start
            } else {
                Handoff::Kept(format!(
                    "{unit} came back when the resolver of {} did not serve with its ports free",
                    record.artefact
                ))
            },
        );
    }
    let restore = |detail: String| Handoff::Restore {
        scopes: record.scopes.clone(),
        artefact: record.artefact.clone(),
        detail,
    };
    Ok(match (published, not_running) {
        (Some(seen), _) if seen.written > record.since && seen.state != "starting" => {
            restore(format!(
                "the resolver in pid {} published {} without its listeners after {unit} \
                 stepped aside",
                seen.pid, seen.state
            ))
        }
        (_, Some(reason)) if stopped => restore(format!("{unit} stepped aside, but {reason}")),
        (_, Some(reason)) => Handoff::Waiting(format!(
            "{unit} stepped aside and {reason} while it restarts; its resolver has not answered"
        )),
        (_, None) => Handoff::Waiting(format!(
            "{unit} stepped aside for the resolver in {}, which has not served yet",
            process.unit
        )),
    })
}

/// Retire `unit` for the replacement `process` and record the scopes withdrawn.
///
/// Recorded twice: before the bootout, so the scopes survive a failure half
/// way, and after it, so the handoff's time follows the old unit's last
/// word. The old unit publishes into the same state file until it is booted
/// out, and none of that may read as the new resolver's answer.
pub async fn start_handoff(
    target: &ComputeTarget,
    unit: &str,
    process: &RunningProgram,
    runner: &Runner,
) -> Result<(String, String), DeployError> {
    let scopes: Vec<String> = label_autostart(target, unit, runner)
        .await?
        .into_iter()
        .filter_map(|(scope, enabled)| enabled.then_some(scope))
        .collect();
    let artefact = &process.resolved;
    write_record(target, unit, "handed_over", &scopes, artefact, runner).await?;
    let (_, detail) = retire_label(target, unit, runner).await?;
    write_record(target, unit, "handed_over", &scopes, artefact, runner).await?;
    Ok((
        "handed_over".to_string(),
        format!("{detail}; the resolver in {} takes its ports", process.unit),
    ))
}

/// Give the withdrawn autostart back and mark `artefact` refused.
pub async fn restore_handoff(
    target: &ComputeTarget,
    unit: &str,
    artefact: &str,
    scopes: &[String],
    runner: &Runner,
) -> Result<(), DeployError> {
    for scope in scopes {
        set_label_autostart(target, unit, scope, true, runner).await?;
    }
    write_record(target, unit, "refused", scopes, artefact, runner).await
}

/// Record how far the replacement's resolver got with the listener:
/// `acquired` once it serves, before the old unit is retired, and `complete`
/// once that retirement is confirmed. Neither is reopened by later resolver
/// states.
pub async fn mark_handoff(
    target: &ComputeTarget,
    unit: &str,
    state: &str,
    process: &RunningProgram,
    runner: &Runner,
) -> Result<(), DeployError> {
    write_record(target, unit, state, &[], &process.resolved, runner).await
}
