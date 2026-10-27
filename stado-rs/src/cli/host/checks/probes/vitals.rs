use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::health::api::beacon_age;
use crate::cli::host::checks::health::beacon_store;
use crate::cli::host::checks::probes::{cell, print_json, report_outcome};

/// `stado host uptime TARGET [--json]` — uptime, load averages and
/// logged-in users (`stado.wisent.com/docs/missing-commands` item two).
pub async fn uptime(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_state::uptime::uptime_host(target, &runner)
        .await
        .map_err(CmdError::from)?;
    if json {
        print_json(&report);
        return report_outcome(&report, crate::deploy::host_state::uptime::OK_STATUS);
    }
    let load = report.get("load_average");
    let field = |key: &str| cell(load.and_then(|value| value.get(key)));
    println!("host:    {}", cell(report.get("host")));
    println!("uptime:  {}", cell(report.get("uptime")));
    println!(
        "load:    {} (1m)  {} (5m)  {} (15m)",
        field("one"),
        field("five"),
        field("fifteen")
    );
    // A report without its user list did not say who is logged in; it was
    // printed as "none logged in", which is a claim about the host.
    match report.get("users").and_then(Value::as_array) {
        None => println!("users:   not reported"),
        Some(users) if users.is_empty() => println!("users:   none logged in"),
        Some(users) => {
            let rows: Vec<Vec<String>> = users
                .iter()
                .map(|user| {
                    vec![
                        cell(user.get("user")),
                        cell(user.get("line")),
                        cell(user.get("since")),
                    ]
                })
                .collect();
            crate::cli::reporting::table::print(&["USER", "LINE", "SINCE"], &rows);
        }
    }
    report_outcome(&report, crate::deploy::host_state::uptime::OK_STATUS)
}

/// `stado host ping TARGET [--json]` combines SSH reachability and beacon
/// freshness in one verdict. SSH access alone does not prove that the host's
/// agent is reporting current state. A beacon store that does not open ends
/// the command at once with that error.
pub async fn ping(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let store = beacon_store().await.map_err(|exc| {
        CmdError::unreachable(format!(
            "stado host ping {target}: the beacon store did not open: {exc}"
        ))
    })?;
    let report = crate::deploy::host_state::ping::ping_host(target, &store, &runner)
        .await
        .map_err(CmdError::from)?;
    let verdict = crate::deploy::host_state::ping::Verdict::Ok.as_str();
    if json {
        print_json(&report);
        return report_outcome(&report, verdict);
    }
    let ssh = report.get("ssh_check");
    let beacon = report.get("beacon");
    let part = |section: Option<&Value>, key: &str| cell(section.and_then(|v| v.get(key)));
    let mut rows = vec![
        vec![
            "ssh".to_string(),
            part(ssh, "status"),
            format!("answered as {}", part(ssh, "host")),
        ],
        vec![
            "beacon".to_string(),
            part(beacon, "status"),
            match beacon.and_then(|value| value.get("error")) {
                Some(Value::String(detail)) => detail.clone(),
                _ => format!(
                    "{} old, reported {}",
                    beacon_age(beacon),
                    part(beacon, "reported_at")
                ),
            },
        ],
    ];
    // A host the local tailnet lists: whether its node key expired, which is
    // why SSH to it only times out, and when a valid key ends.
    if let Some(key) = ssh.and_then(|value| value.get("tailnet_key")) {
        let state = match key.get("expired") {
            Some(Value::Bool(true)) => "expired",
            Some(Value::Bool(false)) => "valid",
            _ => crate::cli::reporting::NOT_REPORTED,
        };
        let detail = match key.get("key_expiry") {
            Some(Value::String(at)) => format!("key expiry {at}"),
            _ => "the tailnet states no key expiry for this node".to_string(),
        };
        rows.push(vec!["tailnet key".to_string(), state.to_string(), detail]);
    }
    crate::cli::reporting::table::print(&["SIGNAL", "STATE", "DETAIL"], &rows);
    println!(
        "\nverdict: {} (the worse of the two signals)",
        cell(report.get("status"))
    );
    report_outcome(&report, verdict)
}

/// `stado host exec TARGET [--json] -- CMD…` — run one approved read-only
/// command (`stado.wisent.com/docs/missing-commands` item six).
///
/// The failure keeps whatever
/// [`crate::deploy::host_exec::ExecRefusal`] already knew about itself:
/// an allowlist refusal reaches the operator as the code it stated, its
/// approved spellings as help beside it, and — when `--json` was asked
/// for — as a document rather than as prose the caller cannot parse.
pub async fn exec(target: &str, words: Vec<String>, json: bool) -> Result<(), CmdError> {
    // Text mode passes the host's own output through as it arrives, so a long
    // command is seen working after its wait line instead of standing silent
    // until it exits; --json keeps stdout for the one receipt document.
    let runner = if json {
        crate::deploy::production_runner()
    } else {
        crate::deploy::relaying_runner()
    };
    let report = crate::deploy::host_exec::exec_host(target, &words, &runner)
        .await
        .map_err(|exc| {
            let mut error = CmdError::click(exc.message).machine_readable(json);
            if let Some(code) = exc.code {
                error = error.stating(code);
            }
            if let Some(help) = exc.help {
                error = error.helping(help);
            }
            error
        })?;
    let expected = crate::deploy::host_exec::OK_STATUS;
    if json {
        print_json(&report);
    }
    // In text mode the host's stdout and stderr were already written through
    // as they arrived; the outcome alone remains to be said.
    report_outcome(&report, expected)
}
