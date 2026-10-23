use crate::deploy::service::*;

/// One marker field, with the `-` a shell writes for "empty" removed.
pub(crate) fn undash(field: &str) -> String {
    let trimmed = field.trim();
    if trimmed == "-" {
        return String::new();
    }
    trimmed.to_string()
}

/// Split one whitespace-delimited marker field into its entries, dropping the
/// `-` a shell writes for "empty".
///
/// The remote programs in this module cannot emit an empty field — a bare
/// empty string between two tabs is indistinguishable from a lost column — so
/// they write `-` and every reader has to undo it. Doing that in one place
/// keeps six call sites from each getting it slightly wrong.
pub(crate) fn split_marker_list(field: &str) -> Vec<String> {
    field
        .split_whitespace()
        .filter(|entry| *entry != "-")
        .map(str::to_string)
        .collect()
}

/// The managed-service record a completed deploy or adopt should be
/// recorded under, built from what the host actually reported rather than
/// from what the operator hoped: the resolved unit id, the resolved path,
/// and the init system that answered.
pub fn record_from_report(
    host: &str,
    host_heuristic: Option<&str>,
    name: &str,
    report: &RemoteReport,
    managed_since: &str,
) -> ManagedService {
    let mut service = if report.kind() == KIND_LAUNCHD {
        launchd_service(
            host,
            &report.unit,
            &report.path,
            SOURCE_REGISTRY,
            managed_since,
        )
    } else {
        systemd_service(
            host,
            &report.unit,
            &report.path,
            SOURCE_REGISTRY,
            managed_since,
        )
    };
    service.name = name.to_string();
    service.host_heuristic = host_heuristic.map(str::to_string);
    service
}

/// One host's tail of a managed unit's logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceLog {
    pub host: String,
    pub unit: String,
    /// The log file, or the journalctl invocation that produced the body.
    pub origin: String,
    pub body: String,
    /// The stderr half of the tail: the file it came from, or the reason
    /// there is none ("absent in plist", "<path> (empty)"). `None` where
    /// the channel merges the streams itself — journalctl already carries
    /// stderr, so Linux tails leave this unset.
    pub error_origin: Option<String>,
    pub error_body: String,
}

impl ServiceLog {
    pub fn to_json(&self) -> Value {
        let mut report = json!({
            "host": self.host,
            "unit": self.unit,
            "origin": self.origin,
            "lines": self.body.lines().collect::<Vec<&str>>(),
        });
        if let Some(error_origin) = &self.error_origin {
            report["error_origin"] = json!(error_origin);
            report["error_lines"] = json!(self.error_body.lines().collect::<Vec<&str>>());
        }
        report
    }
}

/// `service logs` on one host.
pub async fn tail_logs(
    target: &ComputeTarget,
    service: &ManagedService,
    lines: usize,
    runner: &Runner,
) -> Result<ServiceLog, DeployError> {
    tail_unit_logs(
        target,
        service.unit_id(),
        &service.path,
        Some(lines),
        runner,
    )
    .await
}

/// The read that answers a log file's last line holding anything: the
/// script's `stado_last_line`, which reads the file backwards from its end
/// and stops at the first such line, so neither the host nor the connection
/// carries more than the unit's last message. What `stado service status`
/// and an outage show by default.
const LAST_LINE: &str = "stado_last_line";
/// The same stop for the journal, read newest first.
const FIRST_HELD: &str = "/usr/bin/awk 'NF { print; exit }'";
const NEWEST_FIRST: &str = "-r";
/// The read that passes a stream through whole.
const WHOLE: &str = "/bin/cat";

/// How each source of a unit's log is read on the host: its stdout file,
/// its stderr file, and the journal's order, entry count and filter.
struct Reads {
    out: String,
    err: String,
    journal_order: String,
    journal_lines: String,
    journal: String,
}

/// [`tail_logs`] addressed by the launchd label alone: for `service unit logs`,
/// whose caller names a unit the registry may never have declared, so the
/// plist search falls to the remote prelude's LaunchAgents/LaunchDaemons
/// order instead of a declared path. `lines: None` reads the logs whole;
/// `Some(lines)` reads that many of stdout's and that many of stderr's, and
/// that many journal entries.
pub async fn tail_unit_logs(
    target: &ComputeTarget,
    unit_id: &str,
    path: &str,
    lines: Option<usize>,
    runner: &Runner,
) -> Result<ServiceLog, DeployError> {
    let reads = match lines {
        Some(lines) => {
            let tail = format!("/usr/bin/tail -n {}", shlex_quote(&lines.to_string()));
            Reads {
                out: tail.clone(),
                err: tail,
                journal_order: String::new(),
                journal_lines: shlex_quote(&lines.to_string()),
                journal: WHOLE.to_owned(),
            }
        }
        None => Reads {
            out: WHOLE.to_owned(),
            err: WHOLE.to_owned(),
            journal_order: String::new(),
            journal_lines: "all".to_owned(),
            journal: WHOLE.to_owned(),
        },
    };
    read_unit_logs(target, unit_id, path, reads, runner).await
}

/// The last line holding anything of each of the unit's log files, or of
/// its journal, which holds both streams: one reader for every caller that
/// shows only what the unit last said, so none of them states its own
/// window. Each source is read from its end and stops at that line, so a
/// long log costs no more than a short one.
pub async fn last_unit_lines(
    target: &ComputeTarget,
    unit_id: &str,
    path: &str,
    runner: &Runner,
) -> Result<ServiceLog, DeployError> {
    let reads = Reads {
        out: LAST_LINE.to_owned(),
        err: LAST_LINE.to_owned(),
        journal_order: NEWEST_FIRST.to_owned(),
        journal_lines: "all".to_owned(),
        journal: FIRST_HELD.to_owned(),
    };
    let mut log = read_unit_logs(target, unit_id, path, reads, runner).await?;
    log.body = log.body.trim_end_matches('\n').to_owned();
    log.error_body = log.error_body.trim_end_matches('\n').to_owned();
    Ok(log)
}

/// Run the log script with each source's read.
async fn read_unit_logs(
    target: &ComputeTarget,
    unit_id: &str,
    path: &str,
    reads: Reads,
    runner: &Runner,
) -> Result<ServiceLog, DeployError> {
    let body = LOGS_BODY
        .replace("@LINES@", &reads.journal_lines)
        .replace("@JOURNAL_ORDER@", &reads.journal_order)
        .replace("@JOURNAL_READ@", &reads.journal)
        .replace("@OUT_READ@", &reads.out)
        .replace("@ERR_READ@", &reads.err);
    let script = remote_script(unit_id, "", path, &body)?;
    let report = run_remote(target, script, runner).await?;
    let Some((origin, tail)) = split_marker_body(&report.stdout, "STADO_LOG") else {
        return Err(DeployError::unreachable(format!(
            "{}: {} log unavailable: {}",
            target.name,
            unit_id,
            report.failure()
        )));
    };
    let (body, error) = split_error_section(tail);
    let (error_origin, error_body) = match error {
        Some((error_origin, error_body)) => {
            (Some(error_origin.to_string()), error_body.to_string())
        }
        None => (None, String::new()),
    };
    Ok(ServiceLog {
        host: target.name.clone(),
        unit: unit_id.to_string(),
        origin: origin.to_string(),
        body: body.to_string(),
        error_origin,
        error_body,
    })
}
