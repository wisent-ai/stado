//! `stado cloud host reap --provider azure [--apply]`, and the pass the
//! coordinator runs on every tick: readmit the fleet's current addresses to
//! every cloud host, and return each host that has stood idle longer than
//! the minutes it declares.
//!
//! A host is busy while a managed run stands in its run area (every
//! `stado host cargo` run lives there until it ends) or a Cargo or rustc
//! process runs on it. It has stood idle since the last change of its run
//! area, or since it was created when no run ever came. A host whose state
//! cannot be read is reported and kept: an unread host is never judged idle.

use serde_json::{json, Value};

use super::declaration::{self, Declared};
use super::{azure, delete, egress, Azure, ReapArgs};
use crate::cli::registry::{commit_document, fetch_versioned_document};
use crate::cli::CmdError;
use crate::providers::azure::cloud_host;

/// What one host's state read answered.
enum Activity {
    Busy,
    /// Idle since this Unix time.
    IdleSince(i64),
    Unread(String),
}

fn idle_script() -> String {
    let area = crate::deploy::host_run::RUN_AREA;
    format!(
        "runs=\"$HOME/{area}\"\n\
         if pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null; then echo busy; exit 0; fi\n\
         if [ -d \"$runs\" ]; then\n\
           if [ -n \"$(ls -A \"$runs\")\" ]; then echo busy; exit 0; fi\n\
           echo \"idle $(stat -c %Y \"$runs\")\"\n\
         else\n\
           echo never\n\
         fi\n"
    )
}

async fn activity(host: &Declared) -> Activity {
    let created = match chrono::DateTime::parse_from_rfc3339(&host.cloud.created_at) {
        Ok(created) => created.timestamp(),
        Err(error) => {
            return Activity::Unread(format!(
                "its cloud declaration's created_at {:?} is not a time: {error}",
                host.cloud.created_at
            ))
        }
    };
    let target = match crate::deploy::host_channel::canonical_target(&host.name).await {
        Ok(target) => target,
        Err(error) => return Activity::Unread(error.to_string()),
    };
    let runner = crate::deploy::production_runner();
    let output =
        match crate::deploy::host_channel::run_script(&target, &idle_script(), &runner).await {
            Ok(output) if output.succeeded() => output,
            Ok(output) => {
                return Activity::Unread(format!(
                    "its state read exited {}: {}",
                    output.code,
                    output.detail().trim()
                ))
            }
            Err(error) => return Activity::Unread(error.to_string()),
        };
    let answer = output.stdout.trim();
    match answer.split_once(' ') {
        _ if answer == "busy" => Activity::Busy,
        _ if answer == "never" => Activity::IdleSince(created),
        Some(("idle", since)) => match since.parse::<i64>() {
            Ok(since) => Activity::IdleSince(since.max(created)),
            Err(error) => Activity::Unread(format!(
                "its run area's change time {since:?} is not a number: {error}"
            )),
        },
        _ => Activity::Unread(format!("its state read answered {answer:?}")),
    }
}

/// Admit `sources` to every host in `hosts` and to the subnets of their
/// locations.
pub(super) async fn readmit(
    az: &Azure,
    hosts: &[Declared],
    sources: &[String],
) -> Result<(), CmdError> {
    let mut locations: Vec<&str> = hosts
        .iter()
        .map(|host| host.cloud.location.as_str())
        .collect();
    locations.sort_unstable();
    locations.dedup();
    for location in locations {
        cloud_host::admit(
            &az.client,
            az.resource_group,
            az.vnet,
            az.subnet,
            location,
            sources,
        )
        .await
        .map_err(CmdError::refused)?;
    }
    for host in hosts {
        cloud_host::admit_host(&az.client, az.resource_group, &host.name, sources)
            .await
            .map_err(CmdError::refused)?;
    }
    Ok(())
}

/// One pass: readmit, judge each host, and with `apply` delete the idle
/// ones. Returns one row per host.
async fn pass(az: &Azure, apply: bool) -> Result<(Vec<(String, String)>, Vec<Value>), CmdError> {
    let (document, _) = fetch_versioned_document().await?;
    let hosts = declaration::hosts(&document, super::create::PROVIDER)?;
    let admit_from = declaration::admit_from(&document)?;
    let measured = egress::measure(&admit_from).await?;
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    commit_document(|document| Ok(declaration::with_egress(document, &measured, &at))).await?;
    readmit(az, &hosts, &egress::sources(&measured)).await?;
    let now = chrono::Utc::now().timestamp();
    let mut rows = Vec::new();
    for host in &hosts {
        let row = match activity(host).await {
            Activity::Busy => json!({ "target": host.name, "state": "busy" }),
            Activity::Unread(cause) => {
                json!({ "target": host.name, "state": "unread", "cause": cause })
            }
            Activity::IdleSince(since) => {
                let idle_seconds = now - since;
                let limit = host.cloud.idle_minutes * 60;
                if idle_seconds < limit {
                    json!({ "target": host.name, "state": "idle", "idle_seconds": idle_seconds, "idle_limit_seconds": limit })
                } else if !apply {
                    json!({ "target": host.name, "state": "reapable", "idle_seconds": idle_seconds, "idle_limit_seconds": limit })
                } else {
                    match delete::remove(az, &host.name).await {
                        Ok(deleted) => {
                            json!({ "target": host.name, "state": "reaped", "idle_seconds": idle_seconds, "deleted": deleted })
                        }
                        Err(error) => {
                            json!({ "target": host.name, "state": "reap_failed", "idle_seconds": idle_seconds, "cause": error.to_string() })
                        }
                    }
                }
            }
        };
        rows.push(row);
    }
    Ok((measured, rows))
}

pub(super) async fn reap(args: ReapArgs) -> Result<(), CmdError> {
    let super::super::CloudProvider::Azure = args.provider;
    let az = azure()?;
    let (measured, rows) = pass(&az, args.apply).await?;
    let failed = rows.iter().any(|row| row["state"] == "reap_failed");
    let receipt = json!({
        "admitted": measured.iter().map(|(host, address)| json!({ "host": host, "address": address })).collect::<Vec<_>>(),
        "hosts": rows,
        "applied": args.apply,
    });
    if args.json {
        crate::cli::print_answer(&receipt, true)?;
    } else {
        println!(
            "admitted: {}",
            measured
                .iter()
                .map(|(host, address)| format!("{host} {address}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for row in receipt["hosts"].as_array().into_iter().flatten() {
            println!("{row}");
        }
    }
    if failed {
        return Err(CmdError::unreachable(
            "a cloud host stood idle past its limit and could not be deleted; its row says why",
        ));
    }
    Ok(())
}

/// The coordinator's pass: nothing when Azure is not enabled in this profile
/// or no cloud host is declared; otherwise the applied pass, logged.
pub(crate) async fn reap_pass(log: &dyn Fn(&str)) {
    if crate::providers::require_enabled(crate::capabilities::ProviderId::Azure.as_str()).is_err() {
        return;
    }
    let az = match azure() {
        Ok(az) => az,
        Err(error) => {
            log(&format!("cloud-host reaper: {error}"));
            return;
        }
    };
    match fetch_versioned_document().await {
        Ok((document, _)) => match declaration::hosts(&document, super::create::PROVIDER) {
            Ok(hosts) if hosts.is_empty() => return,
            Ok(_) => {}
            Err(error) => {
                log(&format!(
                    "cloud-host reaper: the registry's cloud hosts could not be read: {error}"
                ));
                return;
            }
        },
        Err(error) => {
            log(&format!(
                "cloud-host reaper: the registry could not be read: {error}"
            ));
            return;
        }
    }
    match pass(&az, true).await {
        Ok((_, rows)) => {
            for row in rows {
                if row["state"] == "reaped"
                    || row["state"] == "reap_failed"
                    || row["state"] == "unread"
                {
                    log(&format!("cloud-host reaper: {row}"));
                }
            }
        }
        Err(error) => log(&format!("cloud-host reaper: pass refused: {error}")),
    }
}
