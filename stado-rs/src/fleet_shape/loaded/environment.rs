//! What a unit file hands its program against what the program reads, and the
//! short list of variables launchd supplies without being asked.

use super::super::{Finding, UNIT_ENV_CHECK};
use crate::deploy::service;
use crate::targets::ComputeTarget;

/// Variables launchd hands every job, so a script reading one of these is not
/// reading something its unit file failed to declare.
///
/// Deliberately short. Anything not on it that a script reads and does not
/// default is a variable somebody has to supply, and the unit file is the only
/// thing that can.
const AMBIENT_VARIABLES: [&str; 12] = [
    "HOME", "PATH", "USER", "SHELL", "TMPDIR", "PWD", "OLDPWD", "LANG", "LC_ALL", "IFS", "UID",
    "LOGNAME",
];

/// A unit file declares the variables its own program reads.
///
/// A launchd job inherits almost nothing, so a plist that names none of what
/// its program requires is a unit that cannot work — and it fails on its
/// interval, quietly, forever. `com.wisent.host-health-beacon-collect` on
/// lukasz-macbook carried `HOME` and `PATH` while its program required
/// `STADO_HOST_HEALTH_API_URL`; it failed every five minutes from 12 August
/// into a log nobody read, and the fleet's own beacon age never noticed
/// because the other hosts self-publish.
///
/// The population is scripts under the account's own home — a fleet script,
/// never a system binary — and the subtraction is against
/// [`AMBIENT_VARIABLES`] plus whatever the script defaults for itself, so a
/// script that handles its own absence is not a finding.
pub(in crate::fleet_shape) fn unit_environment(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for unit in loaded {
        if unit.script_reads.is_empty() {
            continue;
        }
        *measured += 1;
        let missing: Vec<&str> = unit
            .script_reads
            .iter()
            .map(String::as_str)
            .filter(|name| !AMBIENT_VARIABLES.contains(name))
            .filter(|name| !unit.script_assigns.iter().any(|set| set == name))
            .filter(|name| !unit.env_keys.iter().any(|given| given == name))
            .collect();
        if missing.is_empty() {
            continue;
        }
        out.push(Finding {
            check: UNIT_ENV_CHECK,
            subject: format!("{}:{}", target.name, unit.label),
            declared: "the unit file declares every variable its program reads".to_string(),
            observed: format!(
                "the plist hands it [{}] and the program reads [{}] without a default",
                if unit.env_keys.is_empty() {
                    "nothing".to_string()
                } else {
                    unit.env_keys.join(" ")
                },
                missing.join(" ")
            ),
            command: format!(
                "stado service env-set {} <KEY> <value> --host {} for each, or stado service ensure {}",
                unit.label, target.name, unit.label
            ),
        });
    }
}
