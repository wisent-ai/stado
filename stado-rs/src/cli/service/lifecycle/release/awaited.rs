//! A restart answers when the restarted unit serves its declared port again,
//! not when launchd accepted the kickstart.
//!
//! A caller that restarts a unit wants the service back. Answering at the
//! kickstart left every caller to guess when the port would answer, and the
//! release workflow guessed with retry counts and sleeps: a slow start failed
//! at an arbitrary second and a broken one was retried for minutes. This reads
//! the unit against its port the way `stado service serving` does, back to
//! back, and stops on what the unit itself does: it serves every declared
//! port, launchd holds no process for it, or launchd started a second process
//! because the first one died. Another job listening on the same port is not a
//! verdict by itself: a listener on another address of that port (a tailnet
//! proxy in front of a loopback API) does not stop the unit from binding its
//! own, and the unit that cannot bind dies, which the reads below see. When it
//! does, the other holder is named in the failure. Elapsed time alone never
//! stops it.

use crate::cli::CmdError;
use crate::deploy::service::ManagedService;
use crate::deploy::service_serving::{self, PORT_SERVED_BY_OTHER, SERVING_YES};
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

/// What the restarted unit did.
pub(crate) enum Served {
    /// Every declared port is held by the unit's own process.
    Serving,
    /// The service directory declares no port for this unit on this host, so
    /// there is nothing to serve and nothing was read.
    NoDeclaredPort,
    /// The unit cannot serve, in the sentence an operator acts on.
    Failed(String),
}

/// Read the restarted unit against its declared port until it serves or
/// shows why it cannot.
pub(crate) async fn until_serving(
    target: &ComputeTarget,
    declared: &ManagedService,
    runner: &Runner,
) -> Result<Served, CmdError> {
    let unit = declared.unit_id();
    let Some(port) =
        crate::cli::service::runtime::serving::directory_port(unit, &declared.host).await
    else {
        return Ok(Served::NoDeclaredPort);
    };
    let ports = [port];
    let mut first_pid: Option<String> = None;
    loop {
        let report = service_serving::read_serving(target, unit, &declared.path, &ports, runner)
            .await
            .map_err(|error| {
                CmdError::from(error).within(format!(
                    "{}: {unit} was restarted, and whether it serves port {port} could not be read",
                    declared.host
                ))
            })?;
        let verdicts = service_serving::port_verdicts(&report);
        if service_serving::verdict(&report, &verdicts) == SERVING_YES {
            return Ok(Served::Serving);
        }
        // Who else holds the port, for the failure sentences below; it decides
        // nothing while the unit's own process is alive.
        let other = verdicts
            .iter()
            .any(|verdict| verdict.verdict == PORT_SERVED_BY_OTHER)
            .then(|| service_serving::failure(&declared.host, &report, &verdicts))
            .flatten();
        let held_by_other = match other {
            Some(said) => format!("; {said}"),
            None => String::new(),
        };
        if report.loaded != "yes" {
            return Ok(Served::Failed(format!(
                "{}: launchd does not hold {unit} after the restart, so it will not serve port {port}",
                declared.host
            )));
        }
        if report.launchd_pid.is_empty() {
            return Ok(Served::Failed(format!(
                "{}: {unit} holds no process after the restart: it exited before serving port \
                 {port}; its own log says why{held_by_other}",
                declared.host
            )));
        }
        match &first_pid {
            None => first_pid = Some(report.launchd_pid.clone()),
            Some(pid) if *pid != report.launchd_pid => {
                return Ok(Served::Failed(format!(
                    "{}: {unit} died before serving port {port}: launchd started pid {} after pid \
                     {pid}; its own log says why{held_by_other}",
                    declared.host, report.launchd_pid
                )));
            }
            Some(_) => {}
        }
    }
}
