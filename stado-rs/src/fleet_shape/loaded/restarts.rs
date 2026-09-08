//! A job's run count and its last exit status, which every other reader on
//! these hosts reported as "loaded".

use super::super::{Finding, RESTART_CHECK};
use crate::deploy::service;
use crate::targets::ComputeTarget;

/// Runs past which a job is not doing work on a schedule, it is looping.
///
/// `stado-resolver` was at 50,863 and `claude-reauth-once` at 45,418 while
/// every other reader called the host healthy, so the threshold only has to be
/// far enough above a legitimate restart count to be beyond argument.
const RESTART_LOOP_RUNS: u64 = 500;

/// A job's run count is work it did, not a loop it is in.
///
/// A one-shot with `KeepAlive` is invisible to every other check here: it
/// reads `active`, it exits, launchd restarts it, forever. Nothing reported
/// the count, so on charless-mac-mini
/// `com.wisent.compute.service.com.wisent.claude-reauth-once` — a job whose
/// own name says `once` — had run 45,418 times and exited 1 every single time,
/// into a log nobody read; `stado-resolver` was at 50,863 and `brama-funnel`
/// at 50,436.
///
/// Two findings, deliberately separate. A job looping is one defect; a job
/// whose last exit is non-zero is another, and a host can have either without
/// the other.
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
        if let Some(runs) = unit.runs {
            *measured += 1;
            if runs >= RESTART_LOOP_RUNS {
                out.push(Finding {
                    check: RESTART_CHECK,
                    subject: format!("{}:{}", target.name, unit.label),
                    declared: format!("a managed job starts fewer than {RESTART_LOOP_RUNS} times"),
                    observed: format!(
                        "launchd has started it {runs} times{}; a one-shot under KeepAlive restarts forever",
                        unit.last_exit
                            .map(|code| format!(", last exit {code}"))
                            .unwrap_or_default()
                    ),
                    command: format!(
                        "stado service label-print {} --host {} then stado host unit-log {} {}",
                        unit.label, target.name, target.name, unit.label
                    ),
                });
                continue;
            }
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
                    command: format!("stado host unit-log {} {}", target.name, unit.label),
                });
            }
        }
    }
}
