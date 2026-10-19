//! A job's run count and its last exit status, which every other reader on
//! these hosts reported as "loaded".

use super::super::{Finding, RESTART_CHECK};
use crate::deploy::service;
use crate::targets::ComputeTarget;

/// The exit status a job reports when it ended successfully.
fn success() -> i64 {
    i64::from(nix::libc::EXIT_SUCCESS)
}

/// How many times the manager has started the job, as a report states it.
fn runs_stated(unit: &service::UndeclaredUnit) -> String {
    match unit.runs {
        Some(runs) => format!(" after {runs} run(s)"),
        None => " (the manager states no run count)".to_string(),
    }
}

/// A job's last exit, with how often launchd has started it.
///
/// A one-shot with `KeepAlive` is invisible to every other check here: it
/// reads `active`, it exits, launchd restarts it, forever. A job whose own
/// name says `once` can run tens of thousands of times and exit 1 every
/// single time, into a log nobody reads. Its failing exit is reported with
/// its run count, so the count shows the loop. No run count is itself called
/// a loop: nobody stated how many starts are too many, and a count alone
/// cannot tell a loop from a job launchd starts on a schedule.
///
/// The one-shot that ends successfully is told apart by its unit file
/// instead: a job launchd keeps alive, that no timer, path or mount starts,
/// and that last ended by itself with success is a job written to finish,
/// started again every time it does.
pub(in crate::fleet_shape) fn restart_loops(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    // A unit counts as measured when the fleet installed it and the manager
    // stated how often it started it.
    *measured += loaded
        .iter()
        .filter(|unit| !unit.declaring_paths.is_empty() && unit.runs.is_some())
        .count();
    for unit in loaded {
        if unit.declaring_paths.is_empty() {
            continue;
        }
        // A non-zero last exit on a job the fleet installed, reported whether
        // or not it is also looping: `78`, `128` and `255` all read as
        // "loaded" to every other command.
        let exit = match last_exit(unit) {
            Ok(exit) => exit,
            Err(observed) => {
                out.push(Finding {
                    check: RESTART_CHECK,
                    subject: format!("{}:{}", target.name, unit.label),
                    declared: "launchd states a job's last exit as a number or `-`".to_string(),
                    observed,
                    command: format!(
                        "stado service unit show {} --host {}",
                        unit.label, target.name
                    ),
                });
                continue;
            }
        };
        match exit {
            Some(code) if code != success() => out.push(Finding {
                check: RESTART_CHECK,
                subject: format!("{}:{}", target.name, unit.label),
                declared: "a managed job's last run succeeded".to_string(),
                observed: format!("last exit {code}{}", runs_stated(unit)),
                command: format!(
                    "stado service unit logs {} --host {} --lines <N>",
                    unit.label, target.name
                ),
            }),
            Some(_) if unit.launch == "keepalive" && unit.last_exit.is_some() => {
                out.push(Finding {
                    check: RESTART_CHECK,
                    subject: format!("{}:{}", target.name, unit.label),
                    declared: "a job launchd keeps alive runs until it is stopped".to_string(),
                    observed: format!(
                        "it ended by itself with success{}; its unit file keeps it alive and \
                         names no schedule, so launchd starts this one-shot again every time it \
                         finishes",
                        runs_stated(unit)
                    ),
                    command: format!(
                        "stado service unit show {} --host {} then stado service unit logs {} \
                         --host {} --lines <N>",
                        unit.label, target.name, unit.label, target.name
                    ),
                });
            }
            _ => {}
        }
    }
}

/// The job's last exit: the posture read's number when it has one, else the
/// status column of `launchctl list`, where `-` (or nothing) is a job that
/// has not exited. Any other word is not an exit status, and it is reported
/// as what launchd said rather than read as no exit.
fn last_exit(unit: &service::UndeclaredUnit) -> Result<Option<i64>, String> {
    if unit.last_exit.is_some() {
        return Ok(unit.last_exit);
    }
    let status = unit.status.trim();
    if status.is_empty() || status == "-" {
        return Ok(None);
    }
    status.parse().map(Some).map_err(|error| {
        format!("launchd reported last exit status {status:?}, which is not a number: {error}")
    })
}
