//! Handing a listener from a role unit to the role inside the product's one
//! process.
//!
//! The old resolver unit and `stado serve --resolver` bind the same ports.
//! While the old unit holds them the new resolver's bind fails, and the
//! supervisor ends the whole `stado serve` over it, so the replacement comes
//! and goes with a new pid each time. A switched-on flag proves nothing; the
//! proof is the resolver's own published state, `serving`, written after the
//! old unit stepped aside by the replacement's current pid.
//!
//! The host keeps one record per unit in `~/.stado/role-handoffs/<unit>`,
//! keyed by the unit and not by a pid, because the replacement's pid does not
//! survive the handoff it is waiting for: when the handoff started, the
//! autostart scopes it withdrew, and the replacement artefact it was for.
//!
//! - No record, and the replacement runs the role: the unit steps aside
//!   (`handed_over`). Only a caller that can bring the unit back starts one.
//! - `handed_over`, and the resolver published `serving` since, by the pid the
//!   replacement runs now: complete, the unit stays retired.
//! - `handed_over`, and it published anything else since, or the replacement
//!   no longer runs the role: the withdrawn scopes are restored and the record
//!   is marked `refused` for that artefact, so the declared unit is repaired.
//! - `handed_over` with nothing published since: the resolver has not tried;
//!   the unit stays out.
//! - `refused`: kept until a different replacement artefact is installed.
//!
//! Every decision follows what the host published; none follows a clock.

use crate::deploy::service::*;

/// The `readiness` a catalog role unit names when its role shares the old
/// unit's listener and only the resolver's published state proves it.
pub const RESOLVER_STATE: &str = "resolver-state";

/// Where a role unit whose role shares its listener stands on one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// The replacement's resolver serves: the unit stays retired.
    Complete,
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

/// The host's record for one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    since: i64,
    state: String,
    scopes: Vec<String>,
    artefact: String,
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
    let serving_now = |since: i64| {
        not_running.is_none()
            && published.as_ref().is_some_and(|(state, pid, written)| {
                state == "serving" && Some(*pid) == current && *written > since
            })
    };
    let Some(record) = record else {
        return Ok(match not_running {
            Some(reason) => Handoff::Kept(reason.to_string()),
            None if serving_now(i64::MIN) => Handoff::Complete,
            None => Handoff::Start,
        });
    };
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
    if serving_now(record.since) {
        return Ok(Handoff::Complete);
    }
    let restore = |detail: String| Handoff::Restore {
        scopes: record.scopes.clone(),
        artefact: record.artefact.clone(),
        detail,
    };
    Ok(match (published, not_running) {
        (Some((state, pid, written)), _) if written > record.since && state != "starting" => {
            restore(format!(
                "the resolver in pid {pid} published {state} after {unit} stepped aside"
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

/// `(state, pid, written epoch)` the resolver last published on `target`, as
/// the replacement's own Stado reads it.
async fn read_published(
    target: &ComputeTarget,
    program: &str,
    runner: &Runner,
) -> Result<Option<(String, u32, i64)>, DeployError> {
    if program.is_empty() {
        return Ok(None);
    }
    let script = format!(
        "p=\"{}\"\nif [ -x \"$p\" ]; then \"$p\" service serve-roles --resolver-state 2>/dev/null || true; fi\n",
        quote_unit_path(program)?
    );
    let output = run(
        target,
        &script,
        "the resolver state could not be read",
        runner,
    )
    .await?;
    Ok(output
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_RESOLVER_STATE", state, pid, written] => Some((
                (*state).trim().to_string(),
                pid.trim().parse().ok()?,
                written.trim().parse().ok()?,
            )),
            _ => None,
        }))
}

async fn read_record(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Result<Option<Record>, DeployError> {
    let script = format!(
        "f=\"$HOME/.stado/role-handoffs/{}\"\nif [ -f \"$f\" ]; then printf 'STADO_ROLE_HANDOFF\\t%s\\n' \"$(/usr/bin/head -n 1 \"$f\")\"; fi\n",
        record_name(unit)?
    );
    let output = run(
        target,
        &script,
        "the handoff record could not be read",
        runner,
    )
    .await?;
    let Some(line) = output
        .lines()
        .find(|line| line.starts_with("STADO_ROLE_HANDOFF\t"))
    else {
        return Ok(None);
    };
    // A record that exists and cannot be read holds scopes only it knows:
    // refusing keeps them, where reading it as absent would start a new
    // handoff over it and lose them.
    let parsed = match host_channel::marker_fields(line).as_slice() {
        ["STADO_ROLE_HANDOFF", since, state, scopes, artefact, "end"] => {
            since.trim().parse().ok().map(|since| Record {
                since,
                state: (*state).trim().to_string(),
                scopes: scopes
                    .split(',')
                    .filter(|scope| !scope.is_empty())
                    .map(str::to_string)
                    .collect(),
                artefact: (*artefact).trim().to_string(),
            })
        }
        _ => None,
    };
    parsed.map(Some).ok_or_else(|| {
        DeployError(format!(
            "{}: the handoff record ~/.stado/role-handoffs/{unit} cannot be read: {line:?}",
            target.name
        ))
    })
}

async fn write_record(
    target: &ComputeTarget,
    unit: &str,
    state: &str,
    scopes: &[String],
    artefact: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let name = record_name(unit)?;
    let script = format!(
        "d=\"$HOME/.stado/role-handoffs\"\n/bin/mkdir -p \"$d\" && printf '%s\\t%s\\t%s\\t%s\\tend\\n' \"$(/bin/date +%s)\" '{state}' '{}' \"{}\" > \"$d/{name}.tmp\" && /bin/mv \"$d/{name}.tmp\" \"$d/{name}\"\n",
        scopes.join(","),
        quote_unit_path(artefact)?
    );
    run(
        target,
        &script,
        "the handoff record could not be written",
        runner,
    )
    .await
    .map(|_| ())
}

async fn run(
    target: &ComputeTarget,
    script: &str,
    failure: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    let output = host_channel::run_script(target, script, runner).await?;
    if output.ok() {
        Ok(output.stdout)
    } else {
        Err(DeployError(host_channel::last_error_line(&output, failure)))
    }
}

/// The record's file name: the exact unit label, which [`validate_unit_id`]
/// holds to characters a shell word and a file name both take as they are.
fn record_name(unit: &str) -> Result<String, DeployError> {
    validate_unit_id(unit)?;
    if unit.contains('/') {
        return Err(DeployError(format!("unit {unit:?} is not one exact label")));
    }
    quote_unit_path(unit)
}
