//! A job launchd still holds after its unit file went away, and the evidence
//! that decides whether it is this fleet's business.

use super::super::{Finding, ORPHAN_CHECK};
use crate::deploy::service;
use crate::targets::ComputeTarget;

/// A job launchd holds has a unit file somebody can read.
///
/// launchd keeps a job after its plist is deleted, and every reader in this
/// binary enumerated unit files or one launchd domain. A job loaded in the
/// system domain with no file on disk was therefore in nobody's list, and that
/// is exactly where the #286 respawner lived: loaded, restarting on
/// `KeepAlive`, invisible, and reported by `list --undeclared`,
/// `list --unowned` and the reap keep-set as "no label holds this".
///
/// The population is the jobs whose PROGRAM comes out of a fleet-managed root
/// — evidence in hand, not the label's spelling. Every `application.com.apple.*`
/// row on a mac is loaded with no unit file too, and it is not this fleet's
/// business.
pub(in crate::fleet_shape) fn loaded_without_unit_file(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for unit in loaded {
        if unit.loaded_domains.is_empty() || !unit.declaring_paths.is_empty() {
            continue;
        }
        *measured += 1;
        let program = if unit.running_program.is_empty() {
            unit.program.as_str()
        } else {
            unit.running_program.as_str()
        };
        if !fleet_program(program) {
            continue;
        }
        out.push(Finding {
            check: ORPHAN_CHECK,
            subject: format!("{}:{}", target.name, unit.label),
            declared: "a loaded job has a unit file in one of the three fleet directories"
                .to_string(),
            observed: format!(
                "launchd holds it in {} with no unit file on disk{}, running {program}",
                unit.loaded_domains.join(", "),
                if unit.path.is_empty() {
                    String::new()
                } else {
                    format!(" (loaded from {}, now gone)", unit.path)
                }
            ),
            command: format!(
                "stado service bootout {} --host {} --domain {}",
                unit.label,
                target.name,
                unit.loaded_domains
                    .first()
                    .map_or("system", |domain| domain.as_str())
            ),
        });
    }
}

/// Does a program come out of a root this fleet installs into?
///
/// Asked of the path rather than of a label, because the label is the thing
/// that lied in every incident this module records.
fn fleet_program(program: &str) -> bool {
    let first = program
        .split_whitespace()
        .find(|word| word.starts_with('/'));
    let Some(path) = first else { return false };
    [
        ".stado/",
        "/weles/",
        "/Users/Shared/stado",
        "/Users/Shared/jeden",
    ]
    .iter()
    .any(|root| path.contains(root))
}
