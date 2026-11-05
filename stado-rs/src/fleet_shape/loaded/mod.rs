//! The checks over the labels launchd has LOADED: how many unit files declare
//! one, and whether the process under it is the program and the binary the
//! fleet delivered. The two questions that come free with the label read the
//! sweep already does live here; the rest are beside them.

pub(in crate::fleet_shape) mod environment;
pub(in crate::fleet_shape) mod orphans;
pub(in crate::fleet_shape) mod restarts;
pub(in crate::fleet_shape) mod shadows;

use super::{Finding, BINARY_CHECK, DOMAIN_CHECK, PROGRAM_CHECK, SERVICE_COPY_CHECK};
use crate::deploy::service;
use crate::targets::ComputeTarget;

/// The `launch` a unit started on a schedule reports (timer, interval,
/// calendar, path or mount), as the loaded-units reader writes it.
const SCHEDULED: &str = "scheduled";

/// What a unit runs, as a key two units of one service share whatever either
/// is called: the binary its declared program executes, without its
/// directory, and that binary's first subcommand. `stado serve …` under any
/// label is one key; `stado network serve` is another. `None` when the unit
/// declares no program.
fn program_key(unit: &service::UndeclaredUnit) -> Option<String> {
    let line = unit.declared_program();
    let mut words = line.split_whitespace();
    let binary = words.next()?;
    let name = binary.rsplit('/').next().unwrap_or(binary);
    Some(match words.find(|word| !word.starts_with('-')) {
        Some(word) => format!("{name} {word}"),
        None => name.to_string(),
    })
}

/// One service, one unit: no two units this host loads run the same program.
///
/// Decided by what each unit RUNS ([`program_key`]), never by what it is
/// called. A second copy of a service has carried every kind of name — a
/// retired prefix, a role suffix, a forward beside it — and a rule that lists
/// those names misses the next one. Two units whose programs execute the same
/// binary with the same subcommand are two copies, whatever their labels say;
/// only fleet units are compared (a unit file this fleet wrote), so two
/// platform agents sharing a helper are not. Only services are compared: a
/// unit its manager starts on a schedule (`launch == "scheduled"`) is a job
/// that runs and ends, and two such jobs of one program with different work
/// — a sync per surface — are two jobs, not two copies of one.
pub(in crate::fleet_shape) fn duplicate_services(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    let mut by_program: std::collections::BTreeMap<String, Vec<&service::UndeclaredUnit>> =
        std::collections::BTreeMap::new();
    for unit in loaded
        .iter()
        .filter(|unit| !unit.declaring_paths.is_empty() && unit.launch != SCHEDULED)
    {
        let Some(key) = program_key(unit) else {
            continue;
        };
        *measured += 1;
        by_program.entry(key).or_default().push(unit);
    }
    for (program, units) in by_program {
        if units.len() < 2 {
            continue;
        }
        let labels: Vec<&str> = units.iter().map(|unit| unit.label.as_str()).collect();
        let declared: Vec<&str> = units
            .iter()
            .filter(|unit| unit.declared)
            .map(|unit| unit.label.as_str())
            .collect();
        let keep = match declared.as_slice() {
            [one] => (*one).to_string(),
            _ => "the one unit the registry declares for it".to_string(),
        };
        out.push(Finding {
            check: SERVICE_COPY_CHECK,
            subject: format!("{}:{program}", target.name),
            declared: format!("one unit runs {program}: {keep}"),
            observed: format!("{} units run it: {}", units.len(), labels.join(", ")),
            command: format!(
                "stado service bootout <each label but {keep}> --host {} --domain <the domain it is loaded in>",
                target.name
            ),
        });
    }
}

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
        // One declaring file is the healthy case; only several are a finding.
        if !matches!(unit.declaring_paths.as_slice(), [_, _, ..]) {
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
/// Two facts, one read, and a fleet can have both of them wrong on the same
/// host at the same time with nothing looking:
///
/// - The control-plane unit declares `stado coordinator`, and launchd holds
///   a `stado dashboard` under it — a command the product has deleted, whose
///   refresh loop forces a space-reclaim pass at its old cadence and holds the
///   janitor's lock while it does, while every report that reads the unit
///   file agrees with itself.
/// - A process older than the binary it executes is running code nobody
///   shipped. `release version show` already answers this per binary, one
///   service at a time, by hand; a delivery can land and labels still
///   execute the previous version hours later with no sweep saying so.
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
    let mut program_checked = Vec::new();
    let mut binary_checked = Vec::new();
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
            program_checked.push(&unit.label);
        }
        // Only where the fleet holds the unit file. This check reads two
        // process timestamps and its remedy is `service restart`, so its
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
                observed: {
                    let binary = match unit.running_binary() {
                        Some(binary) => binary.to_string(),
                        None => "its binary".to_string(),
                    };
                    match unit.binary_written_after_start() {
                        Some(gap) => format!(
                            "pid {} started, then {binary} was replaced {gap} second(s) later",
                            unit.pid
                        ),
                        None => {
                            format!("pid {} started before {binary} was last replaced", unit.pid)
                        }
                    }
                },
                command: format!(
                    "stado service restart {} --host {}",
                    unit.label, target.name
                ),
            });
        }
        if !unit.declaring_paths.is_empty() && unit.runs_current_binary().is_some() {
            binary_checked.push(&unit.label);
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
        program_checked.len(),
        binary_checked.len()
    ));
}
