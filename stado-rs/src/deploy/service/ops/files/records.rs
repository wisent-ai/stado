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

/// [`tail_logs`] addressed by the launchd label alone: for `service unit logs`,
/// whose caller names a unit the registry may never have declared, so the
/// plist search falls to the remote prelude's LaunchAgents/LaunchDaemons
/// order instead of a declared path. `lines: None` reads the logs whole.
pub async fn tail_unit_logs(
    target: &ComputeTarget,
    unit_id: &str,
    path: &str,
    lines: Option<usize>,
    runner: &Runner,
) -> Result<ServiceLog, DeployError> {
    // With a line count the stdout and stderr tails share it, half each with
    // the odd line going to stdout; each side always gets at least one, so
    // `--lines 1` cannot blank stderr entirely. Without one each file and the
    // journal are read whole.
    let (out_read, err_read, journal_lines) = match lines {
        Some(lines) => {
            let out_lines = lines.saturating_sub(lines / 2).max(1);
            let err_lines = (lines / 2).max(1);
            (
                format!("/usr/bin/tail -n {}", shlex_quote(&out_lines.to_string())),
                format!("/usr/bin/tail -n {}", shlex_quote(&err_lines.to_string())),
                shlex_quote(&lines.to_string()),
            )
        }
        None => (
            "/bin/cat".to_string(),
            "/bin/cat".to_string(),
            "all".to_string(),
        ),
    };
    let body = LOGS_BODY
        .replace("@LINES@", &journal_lines)
        .replace("@OUT_READ@", &out_read)
        .replace("@ERR_READ@", &err_read);
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
