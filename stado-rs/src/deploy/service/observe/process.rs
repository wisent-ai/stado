use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Which artefact the live process is executing
// ---------------------------------------------------------------------------

/// What one unit's live process is running, as the host reported it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunningProgram {
    /// The unit id the host addressed.
    pub unit: String,
    /// The pid under the unit, empty when nothing runs under it.
    pub pid: String,
    /// The program the unit's own file declares.
    pub declared: String,
    /// What that declaration resolves to today, once a `current` link in it
    /// has been followed.
    pub resolved: String,
    /// The executable the process table says the pid is running.
    pub running: String,
    /// When the process started.
    pub started_epoch: Option<i64>,
    /// When the declared program was last written.
    pub declared_written_epoch: Option<i64>,
    /// When the running executable was last written.
    pub running_written_epoch: Option<i64>,
}

impl RunningProgram {
    /// The executable path, or `None` when no process was found to ask about.
    pub fn running_binary(&self) -> Option<&str> {
        if self.running.is_empty() {
            None
        } else {
            Some(self.running.as_str())
        }
    }

    /// Whether the process is executing the artefact the unit's declaration
    /// resolves to, or `None` when that could not be established.
    ///
    /// Two ways for a loaded unit at the declared version to be running code
    /// nobody shipped, and both are production incidents:
    ///
    /// - The executable is not what the declaration resolves to. Brama's
    ///   process kept running an artefact tree that `current` no longer
    ///   pointed at, so the unit, the version on disk and the release were all
    ///   correct and the live process was none of them.
    /// - The file was written after the process started. The Weles worker kept
    ///   serving a `dist` that was replaced 26 seconds into its run: the path
    ///   still matches, and the artefact the process loaded is gone.
    ///
    /// `None` is never folded into either answer, for the reason
    /// `service converge` keeps `unknown` apart from `drifted`: a unit with
    /// nothing running under it, or a host that would not say when a file was
    /// written, has produced no evidence about artefact identity, and
    /// answering `true` there would be the report this field exists to
    /// replace.
    pub fn matches_process(&self) -> Option<bool> {
        if self.running.is_empty() || self.declared.is_empty() {
            return None;
        }
        if self.running != self.declared && self.running != self.resolved {
            return Some(false);
        }
        let started = self.started_epoch?;
        let written = match (self.declared_written_epoch, self.running_written_epoch) {
            (Some(declared), Some(running)) => declared.max(running),
            (Some(epoch), None) | (None, Some(epoch)) => epoch,
            (None, None) => return None,
        };
        Some(written <= started)
    }
}

/// Ask one host what the live process under one managed unit is executing.
pub async fn inspect_process(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RunningProgram, DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, PROCESS_BODY)?;
    let report = run_remote(target, script, runner).await?;
    if !report.succeeded("inspected") {
        return Err(DeployError(format!(
            "{}: could not inspect the process under {}: {}",
            target.name,
            service.unit_id(),
            report.failure()
        )));
    }
    let mut program = parse_process(&report.stdout);
    program.unit = report.unit.clone();
    Ok(program)
}

/// The `STADO_PROCESS` marker. An epoch the host could not read arrives empty
/// and stays `None`: a missing timestamp is the absence of a fact, and zero
/// would compare as 1970 and call every process stale.
fn parse_process(stdout: &str) -> RunningProgram {
    let mut program = RunningProgram::default();
    for line in stdout.lines() {
        if let ["STADO_PROCESS", pid, declared, resolved, running, started, declared_written, running_written] =
            host_channel::marker_fields(line).as_slice()
        {
            program.pid = (*pid).to_string();
            program.declared = (*declared).to_string();
            program.resolved = (*resolved).to_string();
            program.running = (*running).to_string();
            program.started_epoch = started.trim().parse().ok();
            program.declared_written_epoch = declared_written.trim().parse().ok();
            program.running_written_epoch = running_written.trim().parse().ok();
        }
    }
    program
}
