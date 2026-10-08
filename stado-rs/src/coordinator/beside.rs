//! The passes that run beside the coordinator's tick, each on its own thread:
//! disaster-recovery replication and the fleet-shape sweep. Each one, awaited
//! inside the loop, held the next tick back (replication for most of half an
//! hour, the shape sweep for minutes), so the tick's lease reaper and schedule
//! firing ran that much later. The loop reports a finished pass, starts the
//! next, and never waits for one.

use super::log;

type ShapeOutcome = Result<crate::fleet_shape::Sweep, String>;

/// The fleet-shape sweep, one pass at a time, on its own thread, the way
/// [`Replication`] runs: the loop reports a finished pass and starts the
/// next, and never waits for one.
#[derive(Default)]
pub(super) struct ShapeSweep {
    running: Option<std::thread::JoinHandle<ShapeOutcome>>,
}

impl ShapeSweep {
    pub(super) fn advance(&mut self) {
        if let Some(pass) = self.running.take_if(|pass| pass.is_finished()) {
            report_shape(pass.join());
        }
        if self.running.is_some() {
            return;
        }
        let started = std::thread::Builder::new()
            .name("stado-fleet-shape".into())
            .spawn(|| -> ShapeOutcome {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("creating the fleet shape runtime: {error}"))?;
                Ok(runtime.block_on(async {
                    let runner = crate::deploy::production_runner();
                    let mut shape = crate::fleet_shape::sweep(&runner).await;
                    if let Some(finding) = crate::fleet_shape::health_disagreement().await {
                        shape.measured += 1;
                        shape.findings.push(finding);
                    }
                    shape
                }))
            });
        match started {
            Ok(pass) => self.running = Some(pass),
            Err(error) => log(&format!("fleet shape sweep not started: {error}")),
        }
    }
}

fn report_shape(outcome: std::thread::Result<ShapeOutcome>) {
    match outcome {
        Ok(Ok(shape)) => {
            log(&shape.summary());
            for finding in &shape.findings {
                log(&finding.line());
            }
            for (host, reason) in &shape.unreachable {
                log(&format!("fleet shape: {host} not measured — {reason}"));
            }
        }
        Ok(Err(exc)) => log(&format!("fleet shape sweep failed: {exc}")),
        Err(_) => log("fleet shape sweep panicked"),
    }
}

type ReplicationOutcome = Result<Option<crate::queue::copy::CopyReport>, String>;

/// Disaster-recovery replication, one pass at a time, on its own thread.
///
/// Awaited inside the loop, between one tick and the next, a pass that fails
/// many objects can take most of half an hour, so the lease reaper at the
/// head of the tick runs that rarely: a build whose agent restarted stays
/// `running` and its release never publishes. The pass runs beside the tick;
/// the loop reports a finished pass and starts the next, and never waits.
#[derive(Default)]
pub(super) struct Replication {
    running: Option<std::thread::JoinHandle<ReplicationOutcome>>,
}

impl Replication {
    pub(super) fn advance(&mut self) {
        if let Some(pass) = self.running.take_if(|pass| pass.is_finished()) {
            report(pass.join());
        }
        if self.running.is_some() {
            return;
        }
        let started = std::thread::Builder::new()
            .name("stado-dr-replication".into())
            .spawn(|| -> ReplicationOutcome {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("creating the replication runtime: {error}"))?;
                runtime
                    .block_on(crate::queue::copy::replicate_configured_backup())
                    .map_err(|error| error.to_string())
            });
        match started {
            Ok(pass) => self.running = Some(pass),
            Err(error) => log(&format!(
                "disaster-recovery replication not started: {error}"
            )),
        }
    }
}

fn report(outcome: std::thread::Result<ReplicationOutcome>) {
    match outcome {
        Ok(Ok(Some(report))) if report.is_clean() => log("disaster-recovery replication clean"),
        Ok(Ok(Some(report))) => log(&format!(
            "disaster-recovery replication incomplete: {} object(s) failed",
            report.failed()
        )),
        Ok(Ok(None)) => {}
        Ok(Err(exc)) => log(&format!("disaster-recovery replication failed: {exc}")),
        Err(_) => log("disaster-recovery replication panicked"),
    }
}
