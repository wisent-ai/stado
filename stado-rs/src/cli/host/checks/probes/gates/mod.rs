use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;

mod outcome;
mod report;

use outcome::claiming_outcome;
use report::print_report;

/// `stado host gates HOST [--json]` — why this host is claiming nothing, in
/// one payload.
///
/// The exit status follows `claiming`, the way `host ping`'s follows its
/// combined verdict, so `stado space reclaim mini --apply --reason … && stado
/// host gates mini` is a usable sentence and a blocked host cannot be
/// mistaken for a healthy one by a script that only reads status codes.
///
/// The Mac mini sat at roughly 2 GiB free against a 55 GiB policy, its agent
/// published `disk_pressure_unresolved` every tick, it claimed nothing for
/// hours, every release build queued behind it — and no command in this CLI
/// said any of it. This is that sentence.
pub async fn gates(host: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let gates = crate::deploy::host_gates::read_host_gates(host, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()).machine_readable(json))?;
    let report = Value::Object(crate::deploy::host_gates::to_report(&gates));
    if json {
        print_json(&report);
        return claiming_outcome(&gates);
    }
    print_report(&gates);
    claiming_outcome(&gates)
}
