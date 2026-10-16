//! What the two reads are printed as, in JSON and in prose.

use serde_json::{json, Value};

use crate::deploy::service::{ImageIdentity, UnitImageObservation};

pub(super) fn emit(
    before: &UnitImageObservation,
    after: Option<&UnitImageObservation>,
    service: &str,
    json_output: bool,
) {
    // An image or a later read that is absent serializes as null: an Option
    // is null when it holds nothing.
    let identity = |image: Option<&ImageIdentity>| {
        json!(image.map(|image| json!({
            "path": image.path,
            "device": image.device,
            "inode": image.inode,
            "bytes": image.bytes,
            "links": image.links,
        })))
    };
    if json_output {
        let report = json!({
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
            "after": after.map(|row| json!({
                "pid": row.pid,
                "running": identity(row.running.as_ref()),
                "installed": identity(row.installed.as_ref()),
                "agrees": row.agrees(),
            })),
        });
        // A JSON value always prints; its alternate form is the pretty one.
        println!("{report:#}");
        return;
    }
    println!("restarted {service}");
    if let (Some(running), Some(pid)) = (before.running.as_ref(), before.pid) {
        println!("  before  pid {pid} was executing {}", running.describe());
    }
    match after {
        Some(row) => {
            let pid = match row.pid {
                Some(pid) => format!("pid {pid}"),
                None => "no pid".to_string(),
            };
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
