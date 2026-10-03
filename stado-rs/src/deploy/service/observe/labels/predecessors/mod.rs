use crate::deploy::service::*;

mod handoff;
mod listener;
mod record;
mod served_root;
mod takeover;

pub use handoff::*;
pub use listener::{hand_over_role, listener_role, listener_standing};
pub use takeover::{local_target, retire_if_taken_over, take_over_on_start};

/// What retiring one unit a product's process replaced did on one host.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PredecessorRetirement {
    pub unit: String,
    /// `retired` when something was booted out or its autostart withdrawn,
    /// `absent` when the host holds neither, `kept` for a role unit whose
    /// role the replacement's live process is not proven to run,
    /// `handed_over` for one that stepped aside for a resolver that has not
    /// served yet, `awaiting_resolver` while that resolver has not tried
    /// since, `restored` for one given back because it did not, `failed`
    /// otherwise.
    pub state: String,
    pub detail: String,
}

/// Retire, on `target`, the units `found` names ([`predecessors_on`] finds
/// them from what each unit runs): boot each out of whichever launchd domain
/// or systemd manager holds it, and withdraw its persistent autostart in
/// every scope that still has it enabled, so a login, reboot or `launchctl
/// load` of its file cannot start it again beside the process that replaced
/// it. A role unit is retired only when [`role_process`] proves `running`,
/// the replacement's unit on this host, runs every role it did; otherwise it
/// is `kept`, because it is still doing that work. A role that shares the old
/// unit's listener is not retired here at all: it is handed over by the
/// reconciler, under the unit's lease, see [`handoff`]; the API listener's
/// units are retired here only once the host Stado process recorded its
/// takeover, and `kept` until then, see [`takeover`]. A unit that runs the
/// host Stado program with no role is reported `kept` with the reason.
///
/// The unit file itself stays where it is: [`set_label_autostart`] records the
/// init system's own disabled override, which outlives the file and is what
/// both managers consult before starting a job.
pub async fn retire_found(
    target: &ComputeTarget,
    running: &ManagedService,
    found: Predecessors,
    runner: &Runner,
) -> Vec<PredecessorRetirement> {
    let mut retirements = Vec::with_capacity(found.replaced.len() + found.roles.len());
    for unit in &found.replaced {
        retirements.push(retirement(target, unit, runner).await);
    }
    for role in &found.roles {
        if listener_role(role) {
            continue;
        }
        // An API listener unit is retired only once the host Stado process
        // recorded its takeover; from then on every pass enforces it, so a
        // retirement that failed once, or a unit a repair brought back, is
        // retired again until it holds. Without a record it is left alone.
        if crate::deploy::service_catalog::api_role(role) {
            retirements.push(match record::taken_over(target, &role.unit, runner).await {
                Some(_) => retirement(target, &role.unit, runner).await,
                None => PredecessorRetirement {
                    unit: role.unit.clone(),
                    state: "kept".to_string(),
                    detail: format!(
                        "it holds the API listener: only {} retires it, when it starts the API \
                         on the same storage root",
                        running.unit_id()
                    ),
                },
            });
            continue;
        }
        retirements.push(match roles_missing(target, running, role, runner).await {
            Ok(None) => retirement(target, &role.unit, runner).await,
            Ok(Some(reason)) => PredecessorRetirement {
                unit: role.unit.clone(),
                state: "kept".to_string(),
                detail: reason,
            },
            Err(error) => PredecessorRetirement {
                unit: role.unit.clone(),
                state: "kept".to_string(),
                detail: format!("its role could not be checked: {error}"),
            },
        });
    }
    retirements.extend(found.uncovered);
    retirements
}

/// Why `running`'s live process does not run every role `role`'s unit did,
/// or `None` when it runs them all.
async fn roles_missing(
    target: &ComputeTarget,
    running: &ManagedService,
    role: &crate::deploy::service_catalog::RoleUnit,
    runner: &Runner,
) -> Result<Option<String>, DeployError> {
    for flag in std::iter::once(&role.flag).chain(role.also.iter()) {
        if let (_, Some(reason)) = role_process(target, running, flag, runner).await? {
            return Ok(Some(reason));
        }
    }
    Ok(None)
}

/// Whether `role`'s unit is out of the way on `target` and must not be
/// repaired: the replacement runs the role; for a role that shares its
/// listener, the listener was acquired or the unit stepped aside and the
/// resolver has not answered yet; for the API listener, the host Stado
/// process recorded its takeover. `stopped` says the registry holds the
/// replacement stopped. The detail when it is; `None` otherwise, including
/// when that cannot be established.
pub async fn role_retired(
    target: &ComputeTarget,
    running: &ManagedService,
    role: &crate::deploy::service_catalog::RoleUnit,
    stopped: bool,
    runner: &Runner,
) -> Option<String> {
    if listener_role(role) {
        return listener::listener_retired(target, running, role, stopped, runner).await;
    }
    if crate::deploy::service_catalog::api_role(role) {
        return record::taken_over(target, &role.unit, runner).await;
    }
    roles_missing(target, running, role, runner)
        .await
        .ok()?
        .is_none()
        .then(|| format!("{} runs its role ({})", running.unit_id(), role.flag))
}

async fn retirement(target: &ComputeTarget, unit: &str, runner: &Runner) -> PredecessorRetirement {
    match retire_label(target, unit, runner).await {
        Ok((state, detail)) => PredecessorRetirement {
            unit: unit.to_string(),
            state,
            detail,
        },
        Err(error) => PredecessorRetirement {
            unit: unit.to_string(),
            state: "failed".to_string(),
            detail: error.to_string(),
        },
    }
}

/// The live process under `running` on `target`, and why it does not run the
/// role `flag` switches on (`None` when it does): something runs under the
/// unit, it executes the artefact the unit declares, and its kernel argument
/// vector, parsed on that host as `stado serve` parses it, switches that role
/// on. The unit's declared arguments are not evidence: a declaration can name
/// the flag before the process that reads it starts.
pub async fn role_process(
    target: &ComputeTarget,
    running: &ManagedService,
    flag: &str,
    runner: &Runner,
) -> Result<(RunningProgram, Option<String>), DeployError> {
    let process = inspect_process(target, running, runner).await?;
    let unit = running.unit_id();
    let reason = if process.pid.is_empty() {
        Some(format!("nothing runs under {unit}"))
    } else if process.matches_process() != Some(true) {
        Some(format!("{unit} is not proven to run its declared program"))
    } else if !process.serve_roles.iter().any(|role| role == flag) {
        Some(format!("{unit}'s process does not run the {flag} role"))
    } else {
        None
    };
    Ok((process, reason))
}

/// Retire one exact label on `target`: boot it out and withdraw its
/// autostart in every scope. `absent` when the host holds neither.
pub async fn retire_label(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Result<(String, String), DeployError> {
    let (booted, booted_detail) = bootout_label(target, unit, BootoutScope::Any, runner).await?;
    if booted == "refused" {
        return Err(DeployError(format!(
            "{}: {unit} could not be booted out: {booted_detail}",
            target.name
        )));
    }
    let mut withdrawn = Vec::new();
    for (scope, enabled) in label_autostart(target, unit, runner).await? {
        if enabled {
            set_label_autostart(target, unit, &scope, false, runner).await?;
            withdrawn.push(scope);
        }
    }
    if booted == "absent" && withdrawn.is_empty() {
        return Ok(("absent".to_string(), booted_detail));
    }
    let mut detail = Vec::new();
    if booted != "absent" {
        detail.push(format!("booted out {booted_detail}"));
    }
    if !withdrawn.is_empty() {
        detail.push(format!("autostart withdrawn in {}", withdrawn.join(", ")));
    }
    Ok(("retired".to_string(), detail.join("; ")))
}
