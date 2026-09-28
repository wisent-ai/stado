use crate::deploy::service::*;

/// What retiring one catalog-retired unit on one host did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PredecessorRetirement {
    pub unit: String,
    /// `retired` when something was booted out or its autostart withdrawn,
    /// `absent` when the host holds neither, `kept` for a role unit whose
    /// role the replacement's live process is not proven to run, `failed`
    /// otherwise.
    pub state: String,
    pub detail: String,
}

/// Retire every unit the catalog lists as replaced by `replacement` on
/// `target`: boot it out of whichever launchd domain or systemd manager holds
/// it, and withdraw its persistent autostart in every scope that still has it
/// enabled, so a login, reboot or `launchctl load` of its file cannot start it
/// again beside the process that replaced it. A role unit is retired only when
/// [`role_taken_over`] proves `running`, the replacement's unit on this host,
/// runs its role; otherwise it is `kept`, because it is still doing that work.
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
        let kept = match role_taken_over(target, running, &role.flag, runner).await {
            Ok(None) => None,
            Ok(Some(reason)) => Some(reason),
            Err(error) => Some(format!("its role could not be checked: {error}")),
        };
        retirements.push(match kept {
            None => retirement(target, &role.unit, runner).await,
            Some(detail) => PredecessorRetirement {
                unit: role.unit.clone(),
                state: "kept".to_string(),
                detail,
            },
        });
    }
    retirements
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

/// Whether the live process under `running` on `target` runs the role `flag`
/// switches on: something runs under the unit, it executes the artefact the
/// unit declares, and it was started with `flag`. `None` when it does; the
/// reason otherwise. The unit's declared arguments are not evidence: a
/// declaration can name the flag before the process that reads it starts.
pub async fn role_taken_over(
    target: &ComputeTarget,
    running: &ManagedService,
    flag: &str,
    runner: &Runner,
) -> Result<Option<String>, DeployError> {
    let process = inspect_process(target, running, runner).await?;
    let unit = running.unit_id();
    Ok(if process.pid.is_empty() {
        Some(format!("nothing runs under {unit}"))
    } else if process.matches_process() != Some(true) {
        Some(format!("{unit} is not proven to run its declared program"))
    } else if !process.flags.iter().any(|started| started == flag) {
        Some(format!("{unit} was not started with {flag}"))
    } else {
        None
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
