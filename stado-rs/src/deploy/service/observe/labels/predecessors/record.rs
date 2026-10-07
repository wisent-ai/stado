//! The host's record of one listener handoff, and the resolver state it is
//! judged by, both read and written over the host channel.

use crate::deploy::service::*;

/// The host's record for one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Record {
    pub(super) since: i64,
    pub(super) state: String,
    pub(super) scopes: Vec<String>,
    pub(super) artefact: String,
}

/// What the resolver last published on a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Published {
    /// Registry health: `serving`, `starting`, `backing_off` or `failed`.
    pub(super) state: String,
    pub(super) pid: u32,
    /// When it was written, epoch seconds.
    pub(super) written: i64,
    /// Whether `pid` held its listeners bound when it wrote this, whatever
    /// the registry's health: the one signal of who owns the ports.
    pub(super) listening: bool,
}

/// What the resolver last published on `target`, as the replacement's own
/// Stado reads it.
pub(super) async fn read_published(
    target: &ComputeTarget,
    program: &str,
    runner: &Runner,
) -> Result<Option<Published>, DeployError> {
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
            ["STADO_RESOLVER_STATE", state, pid, written, listening] => Some(Published {
                state: (*state).trim().to_string(),
                pid: pid.trim().parse().ok()?,
                written: written.trim().parse().ok()?,
                listening: listening.trim() == "1",
            }),
            _ => None,
        }))
}

pub(super) async fn read_record(
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
        DeployError::unreachable(format!(
            "{}: the handoff record ~/.stado/role-handoffs/{unit} cannot be read: {line:?}",
            target.name
        ))
    })
}

pub(super) async fn write_record(
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

/// The record state of an API listener unit this host's Stado process
/// retired at API start, written before it retires the unit.
pub(super) const TAKEN_OVER: &str = "taken_over";

/// The record state of a takeover whose retirement failed, so the unit is
/// still the one serving and may be repaired.
pub(super) const WITHDRAWN: &str = "takeover_withdrawn";

/// The takeover this host's Stado process recorded for API listener unit
/// `unit`: the pid that retired it and since when. `None` when none was
/// recorded, or the record cannot be read, so the unit is still repaired.
pub(super) async fn taken_over(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Option<String> {
    let record = read_record(target, unit, runner).await.ok()??;
    (record.state == TAKEN_OVER).then(|| {
        format!(
            "pid {} retired it at API start on the same storage root (epoch {})",
            record.artefact, record.since
        )
    })
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
        Err(DeployError::unreachable(host_channel::last_error_line(
            &output, failure,
        )))
    }
}

/// The record's file name: the exact unit label, which [`validate_unit_id`]
/// holds to characters a shell word and a file name both take as they are.
fn record_name(unit: &str) -> Result<String, DeployError> {
    validate_unit_id(unit)?;
    if unit.contains('/') {
        return Err(DeployError(format!("unit {unit:?} is not one exact label"))
            .stating(crate::primitives::failure::FailureCode::Refused));
    }
    quote_unit_path(unit)
}
