//! Handing a listener from a role unit to the role inside the product's one
//! process.
//!
//! The old resolver unit and `stado serve --resolver` bind the same ports, so
//! the new resolver cannot serve while the old unit runs, and a switched-on
//! flag says nothing about whether it ever will. Such a role is proven only
//! by the resolver's own published state: `serving`, written by the
//! replacement's pid. Reaching it needs the old unit out of the way first, so
//! the handoff spans passes and is decided by what the resolver publishes,
//! never by how long anybody waited:
//!
//! 1. The replacement runs the role and does not serve: the old unit is
//!    retired and the host records the handoff (replacement pid, when, the
//!    autostart scopes withdrawn) in `~/.stado/role-handoffs/<unit>`.
//! 2. A later pass reads `serving` by that pid: the handoff is complete.
//! 3. It reads another state that pid published after the handoff: the
//!    resolver tried with the ports free and did not serve, so the withdrawn
//!    autostart is restored, the record is marked refused, and the
//!    reconciler's repair of the declared unit starts it again.
//! 4. Nothing newer than the handoff: the resolver has not tried yet, and the
//!    unit stays out and unrepaired.
//! 5. A refused record for the same pid keeps the unit running; a new
//!    replacement process, with a new pid, gets a new handoff.

use crate::deploy::service::*;

/// The `readiness` a catalog role unit names when its role shares the old
/// unit's listener and only the resolver's published state proves it.
pub const RESOLVER_STATE: &str = "resolver-state";

/// Where a role unit whose role shares its listener stands on one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// The replacement's resolver serves: the unit stays retired.
    Complete,
    /// The unit still runs and nothing was tried with this pid: step aside.
    Start { pid: u32 },
    /// Handed over; the resolver has published nothing since.
    Waiting(String),
    /// Handed over and the resolver did not serve: bring the unit back.
    Restore {
        pid: u32,
        scopes: Vec<String>,
        detail: String,
    },
    /// This pid was refused before: the unit keeps its work.
    Refused(String),
}

/// Decide from the replacement's live process and the host's record.
pub async fn handoff_standing(
    target: &ComputeTarget,
    process: &RunningProgram,
    unit: &str,
    runner: &Runner,
) -> Result<Handoff, DeployError> {
    let pid: u32 = process.pid.trim().parse().map_err(|_| {
        DeployError(format!(
            "{}: the replacement pid {:?} is not a number",
            target.name, process.pid
        ))
    })?;
    let published = process.resolver_state.as_ref();
    if let Some((state, by, _)) = published {
        if state == "serving" && *by == pid {
            return Ok(Handoff::Complete);
        }
    }
    let record = read_record(target, unit, runner).await?;
    let Some((recorded, since, state, scopes)) = record.filter(|record| record.0 == pid) else {
        return Ok(Handoff::Start { pid });
    };
    if state == "refused" {
        return Ok(Handoff::Refused(format!(
            "{unit} came back after the resolver in pid {recorded} did not serve with its ports free"
        )));
    }
    Ok(match published {
        Some((state, by, written)) if *by == pid && *written > since => Handoff::Restore {
            pid,
            scopes,
            detail: format!(
                "the resolver in pid {pid} published {state} after {unit} stepped aside"
            ),
        },
        _ => Handoff::Waiting(format!(
            "{unit} stepped aside for the resolver in pid {pid}, which has not published since"
        )),
    })
}

/// Retire `unit` for the replacement `pid` and record the scopes withdrawn.
pub async fn start_handoff(
    target: &ComputeTarget,
    unit: &str,
    pid: u32,
    runner: &Runner,
) -> Result<(String, String), DeployError> {
    let scopes: Vec<String> = label_autostart(target, unit, runner)
        .await?
        .into_iter()
        .filter_map(|(scope, enabled)| enabled.then_some(scope))
        .collect();
    let (state, detail) = retire_label(target, unit, runner).await?;
    write_record(target, unit, pid, "handed_over", &scopes, runner).await?;
    let state = if state == "retired" {
        "handed_over".to_string()
    } else {
        state
    };
    Ok((
        state,
        format!("{detail}; the resolver in pid {pid} takes its ports"),
    ))
}

/// Give the withdrawn autostart back and mark the pid refused.
pub async fn restore_handoff(
    target: &ComputeTarget,
    unit: &str,
    pid: u32,
    scopes: &[String],
    runner: &Runner,
) -> Result<(), DeployError> {
    for scope in scopes {
        set_label_autostart(target, unit, scope, true, runner).await?;
    }
    write_record(target, unit, pid, "refused", scopes, runner).await
}

/// `(pid, epoch, state, scopes)` of the host's record for `unit`.
async fn read_record(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Result<Option<(u32, i64, String, Vec<String>)>, DeployError> {
    let script = format!(
        "f=\"$HOME/.stado/role-handoffs/{}\"\nif [ -f \"$f\" ]; then printf 'STADO_ROLE_HANDOFF\\t%s\\n' \"$(/usr/bin/head -n 1 \"$f\")\"; fi\n",
        record_name(unit)?
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the handoff record could not be read",
        )));
    }
    Ok(output
        .stdout
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_ROLE_HANDOFF", pid, since, state, scopes] => Some((
                pid.trim().parse().ok()?,
                since.trim().parse().ok()?,
                (*state).trim().to_string(),
                scopes
                    .split(',')
                    .filter(|scope| !scope.is_empty())
                    .map(str::to_string)
                    .collect(),
            )),
            _ => None,
        }))
}

async fn write_record(
    target: &ComputeTarget,
    unit: &str,
    pid: u32,
    state: &str,
    scopes: &[String],
    runner: &Runner,
) -> Result<(), DeployError> {
    let name = record_name(unit)?;
    let script = format!(
        "d=\"$HOME/.stado/role-handoffs\"\n/bin/mkdir -p \"$d\" && printf '%s\\t%s\\t%s\\t%s\\n' '{pid}' \"$(/bin/date +%s)\" '{state}' '{}' > \"$d/{name}.tmp\" && /bin/mv \"$d/{name}.tmp\" \"$d/{name}\"\n",
        scopes.join(",")
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if output.ok() {
        Ok(())
    } else {
        Err(DeployError(host_channel::last_error_line(
            &output,
            "the handoff record could not be written",
        )))
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
