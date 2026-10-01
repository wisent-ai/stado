//! The artefact a service unit actually executes, against the program the
//! fleet has installed for it.

use serde_json::Value;

use super::super::{Finding, ARTEFACT_CHECK};
use crate::targets::ComputeTarget;

/// A service unit's artefact is not older than the program the fleet has
/// installed for it.
///
/// The gap this closes is one level below every other version check in the
/// product. `service converge` compares the DECLARED products under
/// `$HOME/<root>/<name>` against the registry and reports `in-sync`;
/// `loaded-label-runs-declared-program` compares a unit file against its
/// process and passes when they agree. Neither can see a unit whose program
/// is `$HOME/.stado/services/<label>/current/<platform>/<program>`, because
/// that artefact is versioned by content hash and the declaration names the
/// `current` link rather than what it points at.
///
/// That blind spot can hold the fleet's object API — the store behind the
/// evidence namespace and the release ingress — on an artefact weeks old
/// while the same host's `.stado/bin/stado` is current, `service converge`
/// reads `in-sync`, and the plist and the process agree with each other all
/// day. An artefact that predates the replication guards is why a replica
/// whose replication was switched off keeps being written to, and one that
/// predates the state-file writer is why a state file an operator reads has
/// a writer nobody can name.
pub(in crate::fleet_shape) fn service_artefacts(
    target: &ComputeTarget,
    reading: &Value,
    out: &mut Vec<Finding>,
    notes: &mut Vec<String>,
) {
    let artefacts: Vec<crate::deploy::host_inventory::ServiceArtifact> = reading
        .get("service_artifacts")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let mut compared = 0_usize;
    for artefact in &artefacts {
        // A version the host read from both files answers the question an
        // mtime only gestures at, so it is judged first and the timestamps
        // stand in for a program that cannot be asked.
        if !artefact.artefact_version.is_empty()
            && !artefact.installed_version.is_empty()
            && artefact.artefact_version != artefact.installed_version
        {
            out.push(Finding {
                check: ARTEFACT_CHECK,
                subject: format!("{}:{}", target.name, artefact.label),
                declared: format!("the artefact is {}", artefact.installed_version),
                observed: format!(
                    "current -> {} answers {}",
                    artefact.current_target, artefact.artefact_version
                ),
                command: format!(
                    "stado service release {} --host {} (a service release, then the unit restarts onto it)",
                    artefact.label, target.name
                ),
            });
            compared += 1;
            continue;
        }
        match artefact.at_least_as_new_as_installed() {
            Some(false) => {
                let behind = artefact.seconds_behind_installed().unwrap_or_default();
                out.push(Finding {
                    check: ARTEFACT_CHECK,
                    subject: format!("{}:{}", target.name, artefact.label),
                    declared: format!(
                        "the artefact runs code no older than $HOME/.stado/bin/{}",
                        artefact.program
                    ),
                    observed: format!(
                        "current -> {} is {} day(s) older than the installed {}",
                        artefact.current_target,
                        behind / 86_400,
                        artefact.program
                    ),
                    command: format!(
                        "stado service release {} --host {} (a service release, then the unit restarts onto it)",
                        artefact.label, target.name
                    ),
                });
                compared += 1;
            }
            Some(true) => compared += 1,
            // No mtime for one side: nothing was proven, and saying so is the
            // difference between "every artefact is current" and "no artefact
            // was read".
            None => {}
        }
    }
    notes.push(format!(
        "{}: {} service artefact(s) found, {} compared against their installed program",
        target.name,
        artefacts.len(),
        compared
    ));
}
