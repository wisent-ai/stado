//! Whether anybody can answer a dialog at the host, and what that decides:
//! on a Mac measured headless the upgrade records the measurement, moves
//! Stado's unit into the system domain through the install, and refuses any
//! step that would raise a dialog there instead of waiting for a click.

use crate::cli::CmdError;
use crate::deploy::host_console;
use crate::deploy::service::{self, UnitDomain};
use crate::deploy::Runner;
use crate::targets::{ComputeTarget, ConsoleDeclaration, ConsoleState};

use super::identity::Dialog;
use super::report::{Report, State};

/// The host's console as the upgrade acts on it.
pub(super) struct Console {
    /// The fresh measurement, when the host could be measured.
    pub measured: Option<ConsoleDeclaration>,
    /// The measurement differs from what the registry records, so the
    /// upgrade records it before the install.
    pub record: bool,
}

impl Console {
    /// Nobody answers a dialog at the host: it measured headless.
    pub fn headless(&self) -> bool {
        self.measured
            .as_ref()
            .is_some_and(|measured| measured.state == ConsoleState::Headless)
    }
}

/// Measure the host's console and say it beside the record. A host that is
/// no Mac, or that could not be measured, is acted on as attended, and the
/// line says why.
pub(super) async fn read(
    target: &ComputeTarget,
    darwin: bool,
    report: &mut Report,
    runner: &Runner,
) -> Console {
    if !darwin {
        report.say(
            "console",
            State::Skipped,
            format!("{} is no Mac; no dialog is asked there", target.name),
        );
        return Console {
            measured: None,
            record: false,
        };
    }
    let recorded = match target.console() {
        Ok(recorded) => recorded,
        Err(error) => {
            report.say("console", State::Failed, error.to_string());
            None
        }
    };
    let measured = match host_console::measure(target, runner).await {
        Ok(measured) => measured.declaration,
        Err(error) => {
            report.say(
                "console",
                State::Failed,
                format!(
                    "{} was not measured, so it is treated as attended and every dialog is \
                     listed: {error}",
                    target.name
                ),
            );
            return Console {
                measured: None,
                record: false,
            };
        }
    };
    let record = match &recorded {
        Some(recorded) => !host_console::agrees(recorded, &measured),
        None => measured.state == ConsoleState::Headless,
    };
    let (state, said) = match (&recorded, record) {
        (Some(_), false) => (
            State::Ok,
            format!(
                "{} — {}; the registry records the same",
                measured.state,
                measured.evidence()
            ),
        ),
        (None, false) => (
            State::Ok,
            format!(
                "{} — {}; the registry records no console, which Stado reads as attended",
                measured.state,
                measured.evidence()
            ),
        ),
        (Some(recorded), true) => (
            State::Planned,
            format!(
                "{} — {}; the registry records {} ({}), and the measurement is recorded before \
                 the install",
                measured.state,
                measured.evidence(),
                recorded.state,
                recorded.evidence()
            ),
        ),
        (None, true) => (
            State::Planned,
            format!(
                "{} — {}; the registry records no console, and the measurement is recorded \
                 before the install",
                measured.state,
                measured.evidence()
            ),
        ),
    };
    report.say("console", state, said);
    Console {
        measured: Some(measured),
        record,
    }
}

/// Record the measurement the plan read, before the install renders the
/// host's units from the registry.
pub(super) async fn record(
    target: &ComputeTarget,
    console: &Console,
    report: &mut Report,
) -> Result<(), CmdError> {
    let Some(measured) = console.measured.as_ref().filter(|_| console.record) else {
        return Ok(());
    };
    let generation = crate::cli::host::console::write(&target.name, Some(measured)).await?;
    report.say(
        "console",
        State::Ok,
        format!(
            "recorded {} for {} (generation {generation})",
            measured.state, target.name
        ),
    );
    Ok(())
}

/// Whether the registry declares the host unit on `target` in the system
/// domain already: a launchd daemon, which macOS never asks about the local
/// network.
pub(super) fn host_unit_is_daemon(target: &ComputeTarget) -> Result<bool, CmdError> {
    let host_unit = crate::deploy::service_catalog::host_unit().map_err(CmdError::declaration)?;
    Ok(service::declared_services(target).iter().any(|unit| {
        unit.unit_id() == host_unit && !UnitDomain::from_path(&unit.path).is_per_login()
    }))
}

/// On a headless Mac, the move of Stado's own units into the system domain:
/// said for the host unit the install moves, and refused — with the dialog
/// it leaves behind — when root's steps are not granted or another unit of
/// Stado's would stay a LaunchAgent.
pub(super) async fn units(
    target: &ComputeTarget,
    console: &Console,
    plan: bool,
    report: &mut Report,
    runner: &Runner,
) -> Result<(), CmdError> {
    if !console.headless() {
        return Ok(());
    }
    let host_unit = crate::deploy::service_catalog::host_unit().map_err(CmdError::declaration)?;
    let agents: Vec<service::ManagedService> = service::declared_services(target)
        .into_iter()
        .filter(|unit| {
            service::is_stado_unit(unit.unit_id())
                && UnitDomain::from_path(&unit.path).is_per_login()
        })
        .collect();
    let local_network = "“\"stado\" would like to find and connect to devices on your local \
                         network.”";
    if let Some(other) = agents.iter().find(|unit| unit.unit_id() != host_unit) {
        return Err(CmdError::refused(format!(
            "{} is headless, and {} runs Stado there as the LaunchAgent {}, which the install \
             does not move: macOS would ask {local_network} at its screen, and nobody is there \
             to answer; move it first with `stado service ensure {} --host {} --as-daemon \
             --reason \"nobody answers macOS at this host's screen\"`; nothing was changed",
            target.name,
            other.unit_id(),
            other.path,
            other.name,
            target.name
        )));
    }
    let Some(agent) = agents.first() else {
        report.say(
            "units",
            State::Ok,
            format!(
                "{host_unit} runs as a launchd daemon on {}, which macOS never asks about the \
                 local network",
                target.name
            ),
        );
        return Ok(());
    };
    let refused = service::refused_daemon_steps(target, &host_unit, runner)
        .await
        .map_err(CmdError::from)?;
    if !refused.is_empty() {
        let steps: Vec<String> = refused
            .iter()
            .map(|step| format!("{} ({})", step.command, step.said))
            .collect();
        return Err(CmdError::refused(format!(
            "{} is headless, and moving {host_unit} from the LaunchAgent {} into the system \
             domain needs root there, which `sudo -n` refuses: {}; as a LaunchAgent the new \
             program would make macOS ask {local_network} at a screen nobody answers; nothing was \
             changed",
            target.name,
            agent.path,
            steps.join("; ")
        )));
    }
    report.say(
        "units",
        if plan { State::Planned } else { State::Ok },
        format!(
            "the install moves {host_unit} from the LaunchAgent {} to {}/{host_unit}.plist, run \
             as the host's account: the agent is stopped, the daemon is read until it serves its \
             declared port, then the agent's file is removed; a daemon that does not answer is \
             removed and the agent loaded again",
            agent.path,
            service::LAUNCHD_DAEMONS
        ),
    );
    Ok(())
}

/// On a headless Mac, the refusal naming every dialog the upgrade would
/// raise there and its cause; `None` when it raises none there.
pub(super) fn refuse_dialogs(
    target: &ComputeTarget,
    console: &Console,
    dialogs: &[Dialog],
) -> Option<CmdError> {
    let measured = console.measured.as_ref().filter(|_| console.headless())?;
    let there: Vec<String> = dialogs
        .iter()
        .filter(|dialog| dialog.host == target.name)
        .map(|dialog| {
            format!(
                "“{}” {}, because {}",
                dialog.says, dialog.when, dialog.because
            )
        })
        .collect();
    if there.is_empty() {
        return None;
    }
    Some(CmdError::refused(format!(
        "{} is headless ({}), and the upgrade would make macOS ask {}; nobody is at its screen \
         to answer, so nothing was changed",
        target.name,
        measured.evidence(),
        there.join("; and ")
    )))
}
