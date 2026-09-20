//! `stado host disk-cleanup TARGET` — run the registry-authorized janitor on
//! another host and report what it freed.
//!
//! The janitor already exists: `stado disk-cleanup` enforces the cleanup
//! policy the registry declares, and `stado install-disk-cleanup` installs
//! its watch. Both are local-only — they act on the Mac you are sitting at —
//! so a fleet host that drifts below its low watermark cannot be brought back
//! from anywhere.
//!
//! On 2026-09-20 that closed the fleet's only browser host: charless-mac-mini
//! published 6.7 GiB free against an 8 GiB low watermark, Stado refused every
//! placement on it with `disk_pressure_active`, and every Weles workload —
//! including the deployment that would move its worker to a fixed revision —
//! was refused with the same sentence. Nothing in the product could clear it,
//! and no operator gesture should have to: the host runs its own Stado, and
//! this command is the fleet-side way to make it run its own policy.
//!
//! Nothing about what may be deleted is decided here. The remote binary reads
//! the same declaration it reads when a person runs it on that machine; this
//! command chooses the host, the pass, and nothing else.

use serde_json::{json, Map, Value};

use crate::cli::host::checks::probes::{print_json, report_outcome};
use crate::cli::CmdError;
use crate::deploy::host_channel;

/// The host's own managed Stado. A cleanup pass must be the build that host
/// runs, never one pushed across for the occasion.
const REMOTE_STADO: &str = "~/.stado/bin/stado";

/// The verdict a completed pass reports.
pub const OK_STATUS: &str = "cleanup_complete";

/// `stado host disk-cleanup TARGET [--dry-run] [--json]`.
///
/// `--dry-run` plans a pass and deletes nothing, which is the same flag the
/// local command carries. Without it the pass runs toward the declared
/// target rather than stopping at the low watermark, because a host that is
/// already refusing placements needs headroom, not the minimum.
pub async fn disk_cleanup(target: &str, dry_run: bool, json: bool) -> Result<(), CmdError> {
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let pass = if dry_run { "--dry-run" } else { "--to-target" };
    let command = format!("{REMOTE_STADO} disk-cleanup {pass}");
    let output = host_channel::run_command(&resolved, &command, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut fields = Map::new();
    fields.insert("host".to_string(), json!(resolved.name));
    fields.insert("command".to_string(), json!(command));
    fields.insert("dry_run".to_string(), json!(dry_run));
    fields.insert("stdout".to_string(), json!(output.stdout.trim()));
    fields.insert("stderr".to_string(), json!(output.stderr.trim()));
    if !output.ok() {
        let detail = host_channel::last_error_line(&output, "no output");
        fields.insert("status".to_string(), json!("cleanup_refused"));
        fields.insert("detail".to_string(), json!(detail));
        if json {
            print_json(&Value::Object(fields));
        } else {
            println!("host:    {}", resolved.name);
            println!("run:     {command}");
            println!("refused: {detail}");
        }
        return Err(CmdError::click(format!(
            "{}: the cleanup pass was refused: {detail}",
            resolved.name
        )));
    }
    fields.insert("status".to_string(), json!(OK_STATUS));
    let report = Value::Object(fields);
    if json {
        print_json(&report);
        return report_outcome(&report, OK_STATUS);
    }
    println!("host:    {}", resolved.name);
    println!("run:     {command}");
    let body = output.stdout.trim();
    println!(
        "report:\n{}",
        if body.is_empty() { "(no output)" } else { body }
    );
    report_outcome(&report, OK_STATUS)
}
