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
    tail_unit_logs(target, service.unit_id(), &service.path, lines, runner).await
}

/// The most log bytes one read may pull across the host channel.
///
/// A cap in lines cannot bound a transfer and cannot bound a diagnosis: on
/// 2026-09-05 `stado host unit-log … --lines 40000` returned 500 lines of the
/// object API's request log, which covered a few minutes, and an event at
/// 13:17 was already unreadable at 14:10. Bytes bound the transfer honestly.
pub const LOG_WINDOW_BYTES: usize = 4 * 1024 * 1024;

/// [`tail_logs`] addressed by the launchd label alone: for `host unit-log`,
/// whose caller names a unit the registry may never have declared, so the
/// plist search falls to the remote prelude's LaunchAgents/LaunchDaemons
/// order instead of a declared path.
pub async fn tail_unit_logs(
    target: &ComputeTarget,
    unit_id: &str,
    path: &str,
    lines: usize,
    runner: &Runner,
) -> Result<ServiceLog, DeployError> {
    // The stdout and stderr tails share the --lines budget, half each with
    // the odd line going to stdout; each side always gets at least one, so
    // `--lines 1` cannot blank stderr entirely.
    let out_lines = lines.saturating_sub(lines / 2).max(1);
    let err_lines = (lines / 2).max(1);
    let body = LOGS_BODY
        .replace("@LINES@", &shlex_quote(&lines.to_string()))
        .replace("@OUT_LINES@", &shlex_quote(&out_lines.to_string()))
        .replace("@ERR_LINES@", &shlex_quote(&err_lines.to_string()))
        // Bounded by bytes, not by trust in the line count: a unit that
        // writes a request per line fills any line budget in minutes, and a
        // reader who asks for more lines than the host hands back cannot tell
        // a quiet unit from a truncated window. 4 MiB is the ceiling on what
        // crosses the channel; the line count still selects within it.
        .replace("@MAX_BYTES@", &shlex_quote(&LOG_WINDOW_BYTES.to_string()));
    let script = remote_script(unit_id, "", path, &body)?;
    let report = run_remote(target, script, runner).await?;
    let Some((origin, tail)) = split_marker_body(&report.stdout, "STADO_LOG") else {
        return Err(DeployError(format!(
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
