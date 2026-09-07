//! The two repairs that act on software Stado itself declared.
//!
//! Both are narrow on purpose. `restart_unit` restarts a unit the registry
//! names, and only when that unit has no live process — the state the
//! charless-mac-mini pre-check runner was in on 2026-09-06, where launchd
//! kept trying and every attempt died with `Failed to create CoreCLR,
//! HRESULT: 0x8007000C` and exit 137 because the machine could not give the
//! runtime its heap. `reap_recovery` runs a host-recovery program this fleet
//! already ships, by name, and records what that program said.
//!
//! Neither infers a subject. A repair with no declared subject is refused by
//! validation before it ever reaches this module, so there is no code path
//! here that picks something to restart.

use std::io::Write;
use std::process::{Command, Stdio};

use super::report::RepairReport;
use super::schema::MemoryRepairPolicy;

/// The recovery programs a declaration may name, and their exact bytes.
///
/// A map rather than a path lookup: the payload is compiled into the binary
/// that runs it, so a declaration cannot name a script that is not in this
/// release, and the pass cannot be pointed at an operator-supplied file.
pub const RECOVERY_PROGRAMS: [(&str, &str); 3] = [
    (
        "recover-skarbiec-crypto",
        include_str!("../../../host_payloads/recover-skarbiec-crypto.sh"),
    ),
    (
        "recover-skarbiec-audit-lock",
        include_str!("../../../host_payloads/recover-skarbiec-audit-lock.sh"),
    ),
    (
        "recover-skarbiec-acquisition-state",
        include_str!("../../../host_payloads/recover-skarbiec-acquisition-state.sh"),
    ),
];

/// The program bytes a declared recovery name resolves to.
pub fn recovery_program(name: &str) -> Option<&'static str> {
    RECOVERY_PROGRAMS
        .iter()
        .find(|(declared, _)| *declared == name)
        .map(|(_, program)| *program)
}

fn note(report: &mut RepairReport, reason: &str) {
    *report.skipped.entry(reason.to_string()).or_insert(0) += 1;
}

/// Whether a declared unit currently has a live process, and in which domain
/// launchd or systemd holds it.
///
/// The domain is read rather than assumed: a fleet unit may be a per-user
/// agent or a system daemon, and the restart verb differs. A unit nothing
/// reports is not restarted — this pass does not create units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitState {
    /// Loaded in this domain with a live process.
    Running(String),
    /// Loaded in this domain with no live process.
    Idle(String),
    /// No domain reports the unit.
    Unknown,
}

fn macos_domains() -> Vec<String> {
    let uid = crate::providers::local::disk_cleanup::euid();
    vec![format!("gui/{uid}"), "system".to_string()]
}

/// Read one declared unit's state from the host's own service manager.
pub fn unit_state(label: &str) -> UnitState {
    if cfg!(target_os = "macos") {
        for domain in macos_domains() {
            let output = Command::new("/bin/launchctl")
                .args(["print", &format!("{domain}/{label}")])
                .output();
            let Ok(output) = output else { continue };
            if !output.status.success() {
                continue;
            }
            let text = String::from_utf8_lossy(&output.stdout);
            let running = text
                .lines()
                .any(|line| line.trim_start().starts_with("pid = "));
            return if running {
                UnitState::Running(domain)
            } else {
                UnitState::Idle(domain)
            };
        }
        return UnitState::Unknown;
    }
    let loaded = Command::new("systemctl")
        .args(["is-enabled", label])
        .output();
    let Ok(loaded) = loaded else {
        return UnitState::Unknown;
    };
    if !loaded.status.success() {
        return UnitState::Unknown;
    }
    let active = Command::new("systemctl")
        .args(["is-active", label])
        .output();
    let running = active.is_ok_and(|output| output.status.success());
    if running {
        UnitState::Running("system".to_string())
    } else {
        UnitState::Idle("system".to_string())
    }
}

fn restart_unit_in(domain: &str, label: &str) -> Result<(), String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("/bin/launchctl")
            .args(["kickstart", "-k", &format!("{domain}/{label}")])
            .output()
    } else {
        Command::new("systemctl").args(["restart", label]).output()
    };
    let output = output.map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

/// Restart declared units whose process is absent.
///
/// `enforce` decides whether the restart is issued; the examination, the
/// eligibility and the subject list are identical in `report` mode, which is
/// what makes an undeclared or unarmed host's report worth reading.
pub fn restart_units(
    policy: &MemoryRepairPolicy,
    enforce: bool,
    budget: &mut i64,
    log_fn: &mut dyn FnMut(&str),
) -> (RepairReport, Vec<String>) {
    let mut report = RepairReport {
        subjects: policy.units.clone(),
        ..RepairReport::default()
    };
    let mut errors = Vec::new();
    for label in &policy.units {
        report.examined += 1;
        match unit_state(label) {
            UnitState::Running(_) => note(&mut report, "unit_running"),
            UnitState::Unknown => note(&mut report, "unit_not_loaded"),
            UnitState::Idle(domain) => {
                report.eligible += 1;
                if !enforce {
                    note(&mut report, "report_only");
                    continue;
                }
                if *budget <= 0 {
                    note(&mut report, "budget_reached");
                    continue;
                }
                *budget -= 1;
                log_fn(&format!("memory: restarting declared unit {label}"));
                match restart_unit_in(&domain, label) {
                    Ok(()) => report.repaired += 1,
                    Err(detail) => {
                        note(&mut report, "restart_refused");
                        errors.push(format!("{label}: {detail}"));
                    }
                }
            }
        }
    }
    (report, errors)
}

/// Run the declared host-recovery program.
///
/// The program decides for itself whether its own precondition holds —
/// `recover-skarbiec-crypto` refuses unless Skarbiec reported a GPG timeout
/// or a keybox lock — and this repair records that refusal as a skip rather
/// than overruling it. Memory pressure is a reason to ASK the declared
/// recovery to run, never a reason to reap somebody else's daemons.
pub fn run_recovery(
    policy: &MemoryRepairPolicy,
    enforce: bool,
    budget: &mut i64,
    log_fn: &mut dyn FnMut(&str),
) -> (RepairReport, Vec<String>) {
    let mut report = RepairReport::default();
    let mut errors = Vec::new();
    let Some(name) = policy.recovery.as_deref() else {
        note(&mut report, "no_recovery_declared");
        return (report, errors);
    };
    report.subjects.push(name.to_string());
    report.examined += 1;
    let Some(program) = recovery_program(name) else {
        note(&mut report, "recovery_not_in_this_release");
        errors.push(format!("{name}: this build ships no such recovery program"));
        return (report, errors);
    };
    report.eligible += 1;
    if !enforce {
        note(&mut report, "report_only");
        return (report, errors);
    }
    if *budget <= 0 {
        note(&mut report, "budget_reached");
        return (report, errors);
    }
    *budget -= 1;
    log_fn(&format!("memory: running declared recovery {name}"));
    let output = run_program(program);
    match output {
        Ok(output) if output.status.success() => report.repaired += 1,
        Ok(output) => {
            note(&mut report, "recovery_refused");
            errors.push(format!(
                "{name}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Err(error) => {
            note(&mut report, "recovery_refused");
            errors.push(format!("{name}: {error}"));
        }
    }
    (report, errors)
}

/// Run a compiled-in recovery program by handing its bytes to `/bin/sh` on
/// standard input.
///
/// The program never lands on disk: it is release bytes, not host state, and
/// a file written for the moment of the repair is a file somebody can edit
/// between the write and the run.
fn run_program(program: &str) -> std::io::Result<std::process::Output> {
    let mut child = Command::new("/bin/sh")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("recovery shell refused its input"))?
        .write_all(program.as_bytes())?;
    child.wait_with_output()
}
