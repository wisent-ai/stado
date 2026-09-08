//! Turn `--target` / `--all` into the registry rows the run will touch.

use crate::deploy::{py_str_repr, DeployError};
use crate::targets::ComputeTarget;

use super::validate::validate_ssh_target;

/// Python `_select_targets`: exactly one of --target / --all; every
/// selected target must be kind=local with a safe SSH destination.
pub fn select_targets<'a>(
    targets: &[&'a ComputeTarget],
    names: &[String],
    all_targets: bool,
) -> Result<Vec<&'a ComputeTarget>, DeployError> {
    // Python: `if bool(names) == all_targets`.
    if names.is_empty() != all_targets {
        return Err(DeployError(
            "provide one or more --target values, or --all, but not both".to_string(),
        ));
    }

    let selected: Vec<&ComputeTarget> = if !names.is_empty() {
        let mut selected = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut missing: Vec<&str> = Vec::new();
        for name in names {
            match targets.iter().find(|t| t.name == *name) {
                Some(target) => {
                    if !seen.contains(&name.as_str()) {
                        seen.push(name);
                        selected.push(*target);
                    }
                }
                None => missing.push(name),
            }
        }
        if !missing.is_empty() {
            return Err(DeployError(format!(
                "registry target not found: {}",
                missing.join(", ")
            )));
        }
        selected
    } else {
        let selected: Vec<&ComputeTarget> = targets
            .iter()
            .filter(|target| {
                target.is_provider(crate::capabilities::ProviderId::Local)
                    && target.has_ssh_connection()
            })
            .copied()
            .collect();
        if selected.is_empty() {
            return Err(DeployError(
                "registry contains no SSH-managed local targets".to_string(),
            ));
        }
        selected
    };

    for target in &selected {
        if !target.is_provider(crate::capabilities::ProviderId::Local) {
            return Err(DeployError(format!(
                "target {} is not kind=local",
                py_str_repr(&target.name)
            )));
        }
        if !target.has_ssh_connection() {
            return Err(DeployError(format!(
                "target {} has no SSH connection path",
                py_str_repr(&target.name)
            )));
        }
        for (_, destination) in target.ssh_connections() {
            validate_ssh_target(destination)?;
        }
    }
    Ok(selected)
}
