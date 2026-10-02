use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;

mod outcome;
mod report;

pub use outcome::DiskRequirement;
use outcome::{claiming_outcome, disk_outcome};
use report::print_report;

/// `stado host gates HOST [--json] [--require-disk THRESHOLD]` — why this
/// host is claiming nothing, in one payload.
///
/// The exit status follows `claiming`, the way `host ping`'s follows its
/// combined verdict, so `stado space reclaim HOST --apply --reason … && stado
/// host gates HOST` is a usable sentence and a blocked host cannot be
/// mistaken for a healthy one by a script that only reads status codes.
/// With `--require-disk`, it follows free space against that declared
/// threshold instead: the question a build-capacity gate asks.
///
/// A host can publish a healthy heartbeat while refusing work because disk
/// pressure remains unresolved. The admission reason must therefore be
/// visible alongside free space and the configured policy.
pub async fn gates(
    host: &str,
    json: bool,
    require_disk: Option<DiskRequirement>,
) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let gates = crate::deploy::host_gates::read_host_gates(host, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()).machine_readable(json))?;
    let report = Value::Object(crate::deploy::host_gates::to_report(&gates));
    if json {
        print_json(&report);
    } else {
        print_report(&gates);
    }
    match require_disk {
        Some(requirement) => disk_outcome(&gates, requirement),
        None => claiming_outcome(&gates),
    }
}
