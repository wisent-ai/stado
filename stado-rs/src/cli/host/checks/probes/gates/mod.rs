use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;

mod outcome;
mod report;

use outcome::{claiming_outcome, disk_outcome};
use report::print_report;

/// `stado host gates HOST [--json] [--require-disk]` — why this host is
/// claiming nothing, in one payload.
///
/// The exit status follows `claiming`, the way `host ping`'s follows its
/// combined verdict, so `stado space reclaim HOST --apply --reason … && stado
/// host gates HOST` is a usable sentence and a blocked host cannot be
/// mistaken for a healthy one by a script that only reads status codes.
/// With `--require-disk`, it follows the disk-full rule instead: the
/// question a build-capacity gate asks.
pub async fn gates(host: &str, json: bool, require_disk: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let gates = crate::deploy::host_gates::read_host_gates(host, &runner)
        .await
        .map_err(|exc| CmdError::from(exc).machine_readable(json))?;
    let report = Value::Object(crate::deploy::host_gates::to_report(&gates));
    if json {
        print_json(&report);
    } else {
        print_report(&gates);
    }
    if require_disk {
        disk_outcome(&gates)
    } else {
        claiming_outcome(&gates)
    }
}
