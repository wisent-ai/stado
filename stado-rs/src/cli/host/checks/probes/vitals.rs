use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::health::api::beacon_age;
use crate::cli::host::checks::health::beacon_store;
use crate::cli::host::checks::probes::{cell, print_json, report_outcome};

/// `stado host uptime TARGET [--json]` — uptime, load averages and
/// logged-in users (`stado.wisent.com/docs/missing-commands` item two).
pub async fn uptime(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_uptime::uptime_host(target, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if json {
        print_json(&report);
        return report_outcome(&report, crate::deploy::host_uptime::OK_STATUS);
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
    let users = report
        .get("users")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if users.is_empty() {
        println!("users:   none logged in");
    } else {
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
        crate::cli::table::print(&["USER", "LINE", "SINCE"], &rows);
    }
    report_outcome(&report, crate::deploy::host_uptime::OK_STATUS)
}

/// `stado host ping TARGET [--json]` — ssh reachability and beacon age in
/// one verdict (`stado.wisent.com/docs/missing-commands` item three).
///
/// The exit status follows the COMBINED verdict, so a box answering ssh
/// with a five-day-old beacon fails this command. That is the whole point
/// of it: the July incident's host would have passed an ssh-only ping
/// every one of those five days.
pub async fn ping(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let store = beacon_store().await?;
    let report = crate::deploy::host_ping::ping_host(target, &store, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let verdict = crate::deploy::host_ping::Verdict::Ok.as_str();
    if json {
        print_json(&report);
        return report_outcome(&report, verdict);
    }
    let ssh = report.get("ssh_check");
    let beacon = report.get("beacon");
    let part = |section: Option<&Value>, key: &str| cell(section.and_then(|v| v.get(key)));
    let rows = vec![
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
    crate::cli::table::print(&["SIGNAL", "STATE", "DETAIL"], &rows);
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
    let runner = crate::deploy::production_runner();
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
        return report_outcome(&report, expected);
    }
    // The point of the command is the host's own output, so it is passed
    // through untouched rather than folded into a table.
    print!("{}", cell(report.get("stdout")));
    let stderr = cell(report.get("stderr"));
    if !stderr.is_empty() && stderr != "-" {
        eprint!("{stderr}");
    }
    report_outcome(&report, expected)
}
