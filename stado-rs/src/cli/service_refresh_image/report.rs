//! What the two reads are printed as, in JSON and in prose.

use serde_json::{json, Value};

use crate::deploy::service::{ImageIdentity, UnitImageObservation};

pub(super) fn emit(
    before: &UnitImageObservation,
    after: Option<&UnitImageObservation>,
    service: &str,
    json_output: bool,
) {
    let identity = |image: Option<&ImageIdentity>| {
        image.map_or(Value::Null, |image| {
            json!({
                "path": image.path,
                "device": image.device,
                "inode": image.inode,
                "bytes": image.bytes,
                "links": image.links,
            })
        })
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "unit": before.unit,
                "host": before.host,
                "unit_path": before.unit_path,
                "program": before.program,
                "restarted": service,
                "before": {
                    "pid": before.pid,
                    "running": identity(before.running.as_ref()),
                    "installed": identity(before.installed.as_ref()),
                },
                "after": after.map_or(Value::Null, |row| json!({
                    "pid": row.pid,
                    "running": identity(row.running.as_ref()),
                    "installed": identity(row.installed.as_ref()),
                    "agrees": row.agrees(),
                })),
            }))
            .unwrap_or_default()
        );
        return;
    }
    println!("restarted {service}");
    if let (Some(running), Some(pid)) = (before.running.as_ref(), before.pid) {
        println!("  before  pid {pid} was executing {}", running.describe());
    }
    match after {
        Some(row) => {
            let pid = row
                .pid
                .map_or_else(|| "no pid".to_string(), |pid| format!("pid {pid}"));
            match row.running.as_ref() {
                Some(running) => println!("  after   {pid} is executing {}", running.describe()),
                None => println!("  after   {pid}, and its image could not be read"),
            }
            if let Some(installed) = row.installed.as_ref() {
                println!("  declared {} is {}", installed.path, installed.describe());
            }
        }
        None => println!("  after   nothing is executing that unit's argument vector"),
    }
}
