use crate::deploy::service::*;

/// What retiring one catalog-retired unit on one host did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PredecessorRetirement {
    pub unit: String,
    /// `retired` when something was booted out or its autostart withdrawn,
    /// `absent` when the host holds neither, `failed` otherwise.
    pub state: String,
    pub detail: String,
}

/// Retire every unit the catalog lists as replaced by `replacement` on
/// `target`: boot it out of whichever launchd domain or systemd manager holds
/// it, and withdraw its persistent autostart in every scope that still has it
/// enabled, so a login, reboot or `launchctl load` of its file cannot start it
/// again beside the process that replaced it.
///
/// The unit file itself stays where it is: [`set_label_autostart`] records the
/// init system's own disabled override, which outlives the file and is what
/// both managers consult before starting a job. A host that holds none of the
/// units answers `absent` for each, so running this on every pass is safe.
pub async fn retire_catalog_predecessors(
    target: &ComputeTarget,
    replacement: &crate::deploy::service_catalog::CatalogService,
    runner: &Runner,
) -> Vec<PredecessorRetirement> {
    let mut retirements = Vec::with_capacity(replacement.retired_units.len());
    for unit in &replacement.retired_units {
        retirements.push(match retire_label(target, unit, runner).await {
            Ok((state, detail)) => PredecessorRetirement {
                unit: unit.clone(),
                state,
                detail,
            },
            Err(error) => PredecessorRetirement {
                unit: unit.clone(),
                state: "failed".to_string(),
                detail: error.to_string(),
            },
        });
    }
    retirements
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
