//! `stado release failed` — every Stado release a host of the fleet recorded
//! as failed and rolled back from, read from the hosts' health beacons.
//!
//! A host whose delivered Stado refused its configuration records the release
//! as failed (`release local list` shows the record there) and publishes it in
//! its beacon as `failed_releases`; every delivery to another host refuses it.
//! This verb is the one place an operator reads all of them at once, so a
//! release that rolled back on one host is seen without visiting that host.
//! A beacon that could not read its own retained releases says so, and that
//! host is reported as unreadable rather than as one with no failure.

use clap::Args;
use serde_json::{json, Value};

use crate::cli::CmdError;

#[derive(Args)]
pub struct ReleaseFailedArgs {
    /// Print the hosts and their failed releases as one JSON document.
    #[arg(long)]
    json: bool,
}

/// What one host's beacon says about the releases that failed on it.
enum Published {
    Failed(Vec<Value>),
    Unreadable(String),
}

pub(in crate::cli::release_cmd) async fn failed(args: &ReleaseFailedArgs) -> Result<(), CmdError> {
    let unread = |error: CmdError| {
        CmdError::click(format!(
            "the fleet's failed Stado releases cannot be listed: the hosts' beacons could not be read: {error}"
        ))
    };
    let store = crate::cli::host::beacon_store().await.map_err(unread)?;
    let beacons = crate::cli::registry::beacon_bodies(&store)
        .await
        .map_err(unread)?;
    let mut hosts: Vec<(String, Published)> = Vec::new();
    for (host, beacon) in beacons {
        match beacon.get("failed_releases") {
            Some(Value::Array(failed)) if !failed.is_empty() => {
                hosts.push((host, Published::Failed(failed.clone())))
            }
            Some(Value::Object(object)) => {
                let said = match object.get("unreadable") {
                    Some(Value::String(said)) => said.clone(),
                    _ => Value::Object(object.clone()).to_string(),
                };
                hosts.push((host, Published::Unreadable(said)))
            }
            _ => {}
        }
    }
    if args.json {
        let listed: Vec<Value> = hosts
            .iter()
            .map(|(host, published)| match published {
                Published::Failed(failed) => json!({ "host": host, "failed": failed }),
                Published::Unreadable(said) => json!({ "host": host, "unreadable": said }),
            })
            .collect();
        println!("{}", json!({ "hosts": listed }));
        return Ok(());
    }
    if hosts.is_empty() {
        println!("no host of the fleet publishes a failed Stado release");
    }
    for (host, published) in &hosts {
        match published {
            Published::Failed(failed) => {
                for release in failed {
                    let (version, record) = match (release.get("version"), release.get("record")) {
                        (Some(Value::String(version)), Some(record)) => (version.clone(), record),
                        _ => {
                            println!(
                                "{host}  publishes a failed release it does not name: {release}"
                            );
                            continue;
                        }
                    };
                    println!("{host}  Stado {version} failed and was rolled back: {record}");
                }
            }
            Published::Unreadable(said) => {
                println!("{host}  its beacon could not read its retained releases: {said}")
            }
        }
    }
    Ok(())
}
