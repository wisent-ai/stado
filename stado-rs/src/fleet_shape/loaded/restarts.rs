//! A job's run count and its last exit status, which every other reader on
//! these hosts reported as "loaded".

use super::super::{Finding, RESTART_CHECK};
use crate::deploy::service;
use crate::targets::ComputeTarget;

/// A job's last exit, with how often launchd has started it.
///
/// A one-shot with `KeepAlive` is invisible to every other check here: it
/// reads `active`, it exits, launchd restarts it, forever. A job whose own
/// name says `once` can run tens of thousands of times and exit 1 every
/// single time, into a log nobody reads. Its failing exit is reported with
/// its run count, so the count shows the loop. No run count is itself called
/// a loop: nobody stated how many starts are too many, and a count alone
/// cannot tell a loop from a job launchd starts on a schedule.
pub(in crate::fleet_shape) fn restart_loops(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for unit in loaded {
        if unit.declaring_paths.is_empty() {
            continue;
        }
        if unit.runs.is_some() {
            *measured += 1;
        }
        // A non-zero last exit on a job the fleet installed, reported whether
        // or not it is also looping: `78`, `128` and `255` all read as
        // "loaded" to every other command.
        let exit = unit.last_exit.or_else(|| unit.status.parse().ok());
        if let Some(code) = exit {
            if code != 0 {
                out.push(Finding {
                    check: RESTART_CHECK,
                    subject: format!("{}:{}", target.name, unit.label),
                    declared: "a managed job's last run succeeded".to_string(),
                    observed: format!(
                        "last exit {code}{}",
                        unit.runs
                            .map(|runs| format!(" after {runs} run(s)"))
                            .unwrap_or_default()
                    ),
                    command: format!(
                        "stado service unit logs {} --host {} --lines <N>",
                        unit.label, target.name
                    ),
                });
            }
        }
    }
}
