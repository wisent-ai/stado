//! The checks over the labels launchd has LOADED: how many unit files declare
//! one, and whether the process under it is the program and the binary the
//! fleet delivered. The two questions that come free with the label read the
//! sweep already does live here; the rest are beside them.

pub(in crate::fleet_shape) mod environment;
pub(in crate::fleet_shape) mod orphans;
pub(in crate::fleet_shape) mod restarts;
pub(in crate::fleet_shape) mod shadows;

use super::{Finding, BINARY_CHECK, DOMAIN_CHECK, PROGRAM_CHECK};
use crate::deploy::service;
use crate::targets::ComputeTarget;

/// One label, one declaring domain.
///
/// The launchd domains are read by
/// [`crate::deploy::service::loaded_units`], which reports every unit file it
/// found for a label rather than the first — the change that made this
/// detectable at all.
pub(in crate::fleet_shape) fn duplicate_domains(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
) {
    for unit in loaded {
        if unit.declaring_paths.len() < 2 {
            continue;
        }
        out.push(Finding {
            check: DOMAIN_CHECK,
            subject: format!("{}:{}", target.name, unit.label),
            declared: "one unit file per label".to_string(),
            observed: format!(
                "{} unit files declare it: {}",
                unit.declaring_paths.len(),
                unit.declaring_paths.join(", ")
            ),
            command: format!(
                "stado space file remove {} <the domain that should not own it> then stado service ensure {}",
                target.name, unit.label
            ),
        });
    }
}

/// A loaded label runs the program its own unit file declares, and runs the
/// binary that is on disk now.
///
/// Two facts, one read, and this fleet has had both of them wrong on the same
/// host at the same time. Nothing was looking:
///
/// - `com.wisent.compute.service.stado-local-control-plane` declares
///   `stado coordinator`, and launchd was holding a `stado dashboard` from
///   2026-08-26 under it — a command the product DELETED on 2026-08-19,
///   whose refresh loop forced a disk-cleanup pass every two minutes. Each
///   forced pass stamped the janitor's shared interval, so the queue agent's
///   own pass returned `interval_noop` before reaching a single cleaner, and
///   the always-on mac ran with disk maintenance switched off while every
///   report that read the unit file agreed with itself.
/// - A process older than the binary it executes is running code nobody
///   shipped. `service converge` already answers this per service, one
///   service at a time, by hand; on 2026-08-31 the mini had a delivery land
///   at 07:13Z and labels still executing the previous version hours later,
///   and no sweep said so.
///
/// Both come free with the label read the sweep already does, which is the
/// whole reason to ask them here: the cost of the answer is zero and the cost
/// of not having it was a night.
pub(in crate::fleet_shape) fn process_identity(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    notes: &mut Vec<String>,
) {
    let mut program_checked = 0_usize;
    let mut binary_checked = 0_usize;
    for unit in loaded {
        if unit.runs_declared_program() == Some(false) {
            out.push(Finding {
                check: PROGRAM_CHECK,
                subject: format!("{}:{}", target.name, unit.label),
                declared: unit.declared_program(),
                observed: format!("pid {} runs {}", unit.pid, unit.running_program),
                command: format!(
                    "stado service bootout {} --host {} then stado service ensure {}",
                    unit.label, target.name, unit.label
                ),
            });
        }
        if unit.runs_declared_program().is_some() {
            program_checked += 1;
        }
        // Only where the fleet holds the unit file. This check reads two
        // process timestamps and its remedy is `service converge`, so its
        // population is the units this fleet installed -- evidence it has in
        // hand, not a guess from the label's spelling. Now that the scan
        // enumerates every loaded label, an OS daemon whose binary a system
        // update replaced after boot would otherwise be reported here as a
        // fleet finding, with a remedy that cannot touch it.
        if !unit.declaring_paths.is_empty() && unit.runs_current_binary() == Some(false) {
            out.push(Finding {
                check: BINARY_CHECK,
                subject: format!("{}:{}", target.name, unit.label),
                declared: "the process executes the binary now on disk".to_string(),
                observed: format!(
                    "pid {} started, then {} was replaced {} second(s) later",
                    unit.pid,
                    unit.running_binary().unwrap_or("its binary"),
                    unit.binary_written_after_start().unwrap_or_default()
                ),
                command: format!(
                    "stado service converge {} --host {}",
                    unit.label, target.name
                ),
            });
        }
        if !unit.declaring_paths.is_empty() && unit.runs_current_binary().is_some() {
            binary_checked += 1;
        }
    }
    // A label launchd holds no pid for answers neither question, and saying so
    // is the difference between "every process is right" and "no process was
    // read".
    //
    // The classification counts are here for the same reason. The scan now
    // reads every loaded label rather than only the fleet-prefixed ones, and a
    // count per class is what makes the widening auditable: an operator can see
    // that rows outside the prefix were looked at, and how many, instead of
    // trusting that a filter upstream chose correctly.
    let undeclared = loaded
        .iter()
        .filter(|unit| unit.classification() == "undeclared")
        .count();
    let outside = loaded
        .iter()
        .filter(|unit| unit.classification() == "outside-fleet-prefix")
        .count();
    notes.push(format!(
        "{}: {} loaded label(s) — {} declared, {} undeclared, {} outside the fleet prefix; \
         {} process(es) compared against their declaration, {} against the installed binary",
        target.name,
        loaded.len(),
        loaded.iter().filter(|unit| unit.declared).count(),
        undeclared,
        outside,
        program_checked,
        binary_checked
    ));
}
