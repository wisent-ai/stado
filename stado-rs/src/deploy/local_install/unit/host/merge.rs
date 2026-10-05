//! How one existing `stado serve` unit's environment folds into the single
//! host unit. A disagreement between two replaced units is refused, because
//! the host cannot run both settings at once.

use std::collections::BTreeMap;

use crate::deploy::DeployError;

use super::super::InstallPlan;

pub(super) fn merge_environment(
    values: &mut BTreeMap<String, (String, String)>,
    component: &InstallPlan,
) -> Result<(), DeployError> {
    for (name, value) in &component.env {
        let name = name.as_str();
        if let Some((previous, owner)) = values.get_mut(name) {
            if previous != value {
                // A search path is a list, not a setting: the host process
                // needs every directory any replaced unit searched, in the
                // order they were first named.
                if name == "PATH" {
                    let mut entries: Vec<&str> = previous.split(':').collect();
                    for entry in value.split(':') {
                        if !entries.contains(&entry) {
                            entries.push(entry);
                        }
                    }
                    *previous = entries.join(":");
                    continue;
                }
                return Err(DeployError(format!(
                    "host consolidation cannot merge variable {name}: units {owner} and {} disagree",
                    component.label
                )));
            }
        } else {
            values.insert(name.to_string(), (value.clone(), component.label.clone()));
        }
    }
    Ok(())
}

/// The host unit's environment: the plan's defaults, overridden by every
/// replaced unit's variables, with the plan's Skarbiec identity kept.
///
/// A replaced unit's environment is what its role needs (its storage
/// backend, its interpreter), so it is kept over the plan's defaults. Its
/// Skarbiec identity is not: that is the current configuration's, or the one
/// process keeps the retired identities (`stado-control-plane`,
/// `stado-local-agent`) the units were installed with.
pub(super) fn host_environment(
    planned: Vec<(String, String)>,
    replaced: BTreeMap<String, (String, String)>,
) -> Vec<(String, String)> {
    let identity: Vec<(String, String)> = planned
        .iter()
        .filter(|(name, _)| super::skarbiec_identity(name))
        .cloned()
        .collect();
    let mut merged: BTreeMap<String, String> = planned.into_iter().collect();
    merged.extend(replaced.into_iter().map(|(name, (value, _))| (name, value)));
    merged.extend(identity);
    merged.into_iter().collect()
}
