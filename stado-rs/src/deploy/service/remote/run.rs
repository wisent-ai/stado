use crate::deploy::service::*;

/// Feed one fixed remote program to one host over the shared channel.
pub(crate) async fn run_remote(
    target: &ComputeTarget,
    script: String,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report_from(output))
}

/// Feed one fixed remote program to one host and check, on the host and
/// before the connection closes, that it left behind the state it intended.
///
/// The prelude travels separately from the body because the probe is armed
/// between them: `host_channel::PostCondition::arm` explains why the check
/// cannot simply be appended to a body whose success path is an early
/// `exit`.
pub(crate) async fn run_remote_checked(
    target: &ComputeTarget,
    prelude: &str,
    body: &str,
    postcondition: &host_channel::PostCondition,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let (output, verdict) =
        host_channel::run_checked_script(target, prelude, body, postcondition, runner).await?;
    let mut report = report_from(output);
    report.postcondition = verdict.describe;
    report.postcondition_state = verdict.state;
    report.postcondition_detail = verdict.detail;
    Ok(report)
}

/// One transport result as a report, whether or not the operation declared
/// an end state.
pub(crate) fn report_from(output: CommandOutput) -> RemoteReport {
    let mut report = parse_markers(&output.stdout);
    report.exit_code = output.code;
    if report.status.is_empty() && !output.succeeded() {
        // ssh itself failed (unreachable host, refused key), so there are
        // no markers to read: surface the transport's own last word, the
        // way every other command on this channel does.
        report.status = host_channel::FAILED_STATUS.to_string();
        report.detail = host_channel::last_error_line(&output, "ssh failed");
    }
    report.stdout = output.stdout;
    report
}

/// Fold the `STADO_*` marker lines of stdout into a [`RemoteReport`].
///
/// Same protocol and framing as
/// `deploy/host_recovery.rs::parse_output`; matched with slice patterns so
/// a marker with the wrong arity falls through instead of being mis-read.
pub fn parse_markers(stdout: &str) -> RemoteReport {
    let mut report = RemoteReport::default();
    // A malformed adopt line outlives the probe's own status line after it:
    // the probe saying it probed does not make the words it wrote readable.
    let mut malformed = None;
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_HOST", os, domain, unit, path] => {
                report.os = (*os).to_string();
                report.domain = (*domain).to_string();
                report.unit = (*unit).to_string();
                report.path = (*path).to_string();
            }
            ["STADO_SERVICE", _unit, status, detail] => {
                report.status = (*status).to_string();
                report.detail = (*detail).to_string();
            }
            ["STADO_DOMAIN", domain, status, reason] => {
                report.domain = (*domain).to_string();
                report.domain_status = (*status).to_string();
                report.domain_reason = (*reason).to_string();
            }
            ["STADO_ADOPT", file_state, unit_state] => {
                let file = <UnitFilePresence as serde::Deserialize>::deserialize(Value::String(
                    (*file_state).to_string(),
                ));
                let unit = <UnitLoad as serde::Deserialize>::deserialize(Value::String(
                    (*unit_state).to_string(),
                ));
                match (file, unit) {
                    (Ok(file), Ok(unit)) => {
                        report.file_state = Some(file);
                        report.unit_state = Some(unit);
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        // A word the adopt probe does not write says nothing
                        // about the unit: the report is a failed probe naming
                        // it, not a unit read as absent or unloaded.
                        malformed = Some(format!(
                            "the adopt probe reported unit file {file_state:?} and unit \
                             {unit_state:?}, which it does not write: {error}"
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(detail) = malformed {
        report.status = host_channel::FAILED_STATUS.to_string();
        report.detail = detail;
    }
    report
}

/// Split `stdout` at the first `marker` line, returning that line's single
/// trailing field and everything after it. The commands that carry a body
/// (a log tail, a unit file) announce it with a marker and then stream it
/// raw, so the body needs no framing of its own.
pub fn split_marker_body<'a>(stdout: &'a str, marker: &str) -> Option<(&'a str, &'a str)> {
    let mut rest = stdout;
    loop {
        // A line ends at its newline, or at the end of stdout when it is the
        // last one; then nothing follows it.
        let (line, tail) = match rest.split_once('\n') {
            Some(parts) => parts,
            None => (rest, &rest[rest.len()..]),
        };
        if let Some(field) = line
            .strip_prefix(marker)
            .and_then(|head| head.strip_prefix('\t'))
        {
            return Some((field, tail));
        }
        if tail.is_empty() {
            return None;
        }
        rest = tail;
    }
}

/// Split a log tail at the `STADO_ERR` section the logs program emits after
/// the stdout tail on Darwin. The marker line's field is the stderr file,
/// or the reason there is nothing to show ("absent in plist", "<path>
/// (empty)"); everything after the marker line is the stderr tail. Linux
/// tails carry no such marker — the journal merges the streams — and pass
/// through whole.
pub(crate) fn split_error_section(tail: &str) -> (&str, Option<(&str, &str)>) {
    let Some((before, section)) = tail.split_once("\nSTADO_ERR\t") else {
        return (tail, None);
    };
    // The marker line ends at its newline, or at the end of the tail when it
    // is the last line; then the stderr tail is empty.
    let (error_origin, error_body) = match section.split_once('\n') {
        Some(parts) => parts,
        None => (section, &section[section.len()..]),
    };
    // The stdout tail keeps the newline that ended its last line.
    (&tail[..=before.len()], Some((error_origin, error_body)))
}
