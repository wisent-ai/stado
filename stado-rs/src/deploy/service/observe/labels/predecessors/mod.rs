use crate::deploy::service::*;

mod handoff;

pub use handoff::*;

/// What retiring one catalog-retired unit on one host did.
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

/// Retire every unit the catalog lists as replaced by `replacement` on
/// `target`: boot it out of whichever launchd domain or systemd manager holds
/// it, and withdraw its persistent autostart in every scope that still has it
/// enabled, so a login, reboot or `launchctl load` of its file cannot start it
/// again beside the process that replaced it. A role unit is retired only when
/// [`role_process`] proves `running`, the replacement's unit on this host,
/// runs its role; otherwise it is `kept`, because it is still doing that work.
/// A role that shares the old unit's listener is handed over, see [`handoff`].
///
/// The unit file itself stays where it is: [`set_label_autostart`] records the
/// init system's own disabled override, which outlives the file and is what
/// both managers consult before starting a job. A host that holds none of the
/// units answers `absent` for each, so running this on every pass is safe.
pub async fn retire_catalog_predecessors(
    target: &ComputeTarget,
    replacement: &crate::deploy::service_catalog::CatalogService,
    running: &ManagedService,
    runner: &Runner,
) -> Vec<PredecessorRetirement> {
    let mut retirements = Vec::with_capacity(replacement.retired_units.len());
    for unit in &replacement.retired_units {
        retirements.push(retirement(target, unit, runner).await);
    }
    for role in &replacement.role_units {
        retirements.push(role_retirement(target, running, role, runner).await);
    }
    retirements
}

async fn role_retirement(
    target: &ComputeTarget,
    running: &ManagedService,
    role: &crate::deploy::service_catalog::RoleUnit,
    runner: &Runner,
) -> PredecessorRetirement {
    let unit = role.unit.clone();
    let answer = |state: &str, detail: String| PredecessorRetirement {
        unit: unit.clone(),
        state: state.to_string(),
        detail,
    };
    let process = match role_process(target, running, &role.flag, runner).await {
        Ok(Ok(process)) => process,
        Ok(Err(reason)) => return answer("kept", reason),
        Err(error) => return answer("kept", format!("its role could not be checked: {error}")),
    };
    if role.readiness.as_deref() != Some(RESOLVER_STATE) {
        return retirement(target, &role.unit, runner).await;
    }
    let outcome = match handoff_standing(target, &process, &role.unit, runner).await {
        Ok(Handoff::Complete) => return retirement(target, &role.unit, runner).await,
        Ok(Handoff::Refused(detail)) => return answer("kept", detail),
        Ok(Handoff::Waiting(detail)) => Ok(("awaiting_resolver".to_string(), detail)),
        Ok(Handoff::Start { pid }) => start_handoff(target, &role.unit, pid, runner).await,
        Ok(Handoff::Restore {
            pid,
            scopes,
            detail,
        }) => restore_handoff(target, &role.unit, pid, &scopes, runner)
            .await
            .map(|()| ("restored".to_string(), detail)),
        Err(error) => Err(error),
    };
    match outcome {
        Ok((state, detail)) => answer(&state, detail),
        Err(error) => answer("failed", error.to_string()),
    }
}

/// Whether `role`'s unit is out of the way on `target` and must not be
/// repaired: the replacement runs the role, and either the role needs nothing
/// more, or its resolver serves, or the unit stepped aside for it and the
/// resolver has not answered yet. The detail when it is; `None` otherwise,
/// including when that cannot be established.
pub async fn role_retired(
    target: &ComputeTarget,
    running: &ManagedService,
    role: &crate::deploy::service_catalog::RoleUnit,
    runner: &Runner,
) -> Option<String> {
    let process = role_process(target, running, &role.flag, runner)
        .await
        .ok()?
        .ok()?;
    if role.readiness.as_deref() != Some(RESOLVER_STATE) {
        return Some(format!(
            "{} runs its role ({})",
            running.unit_id(),
            role.flag
        ));
    }
    match handoff_standing(target, &process, &role.unit, runner)
        .await
        .ok()?
    {
        Handoff::Complete => Some(format!("the resolver in {} serves", running.unit_id())),
        Handoff::Waiting(detail) => Some(detail),
        Handoff::Start { .. } | Handoff::Restore { .. } | Handoff::Refused(_) => None,
    }
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

/// The live process under `running` on `target`, when it runs the role
/// `flag` switches on: something runs under the unit, it executes the
/// artefact the unit declares, and its kernel argument vector, parsed on that
/// host as `stado serve` parses it, switches that role on. The reason
/// otherwise. The unit's declared arguments are not evidence: a declaration
/// can name the flag before the process that reads it starts.
pub async fn role_process(
    target: &ComputeTarget,
    running: &ManagedService,
    flag: &str,
    runner: &Runner,
) -> Result<Result<RunningProgram, String>, DeployError> {
    let process = inspect_process(target, running, runner).await?;
    let unit = running.unit_id();
    Ok(if process.pid.is_empty() {
        Err(format!("nothing runs under {unit}"))
    } else if process.matches_process() != Some(true) {
        Err(format!("{unit} is not proven to run its declared program"))
    } else if !process.serve_roles.iter().any(|role| role == flag) {
        Err(format!("{unit}'s process does not run the {flag} role"))
    } else {
        Ok(process)
    })
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
