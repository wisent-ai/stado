//! The report itself: the JSON object both interfaces share, and the table
//! and diagnostics the CLI prints.

pub(in crate::cli::service_converge) mod gates;

use serde_json::{json, Value};

use crate::cli::service_converge::model::receipts::{
    AppliedPass, Refused, Released, Undeliverable, FAILED,
};
use crate::cli::service_converge::model::vocabulary::{Row, PROCESS_DIFFERS, UNDECLARED, UNKNOWN};
use crate::cli::service_converge::model::ServiceConvergeResult;
use crate::cli::CmdError;

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

pub(in crate::cli::service_converge) fn report_json(
    target: &str,
    applied: Option<&AppliedPass>,
    rows: &[Row],
) -> Value {
    let empty = AppliedPass::default();
    let pass = applied.unwrap_or(&empty);
    json!({
        "target": target,
        "state": if rows.is_empty() { UNDECLARED } else { "declared" },
        "applied": applied.is_some(),
        "releases": pass.releases.iter().map(Released::to_json).collect::<Vec<Value>>(),
        "undeliverable": pass
            .undeliverable
            .iter()
            .map(Undeliverable::to_json)
            .collect::<Vec<Value>>(),
        "refused": pass.refused.iter().map(Refused::to_json).collect::<Vec<Value>>(),
        "binaries": rows.iter().map(Row::to_json).collect::<Vec<Value>>(),
    })
}

/// The report on stdout, and whatever `--apply` could not do on stderr.
pub(in crate::cli::service_converge) fn emit(
    result: &ServiceConvergeResult,
    json_output: bool,
) -> Result<(), CmdError> {
    let empty = AppliedPass::default();
    let pass = result.applied.as_ref().unwrap_or(&empty);
    let rows = result.rows.as_slice();
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result.report_json())?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "target={} state={UNDECLARED}: this host declares no managed versions; \
             add them to targets[].managed_versions",
            result.target
        );
        return Ok(());
    }
    for row in rows {
        println!(
            "binary={} version={} root={} unit={} state={} attestation={} receipt={} \
             verdict={} declared={} detail={}",
            row.binary,
            row.installed_cell(),
            row.root,
            row.unit,
            row.state,
            row.attestation,
            row.receipt.replace(' ', "_"),
            row.verdict,
            row.declared,
            row.detail
        );
    }
    // The path is what an operator acts on and is far too long for a column, so
    // it is named here — and only for the rows where it contradicts the
    // declaration, which are the rows that would otherwise read as fine.
    //
    // This used to end "restart it to pick up what is installed", which is the
    // wrong instruction to hand someone at seven in the morning. A stale
    // process is a fact, not a fault. On 2026-08-31 `com.wisent.stado-resolver`
    // on charless-mac-mini reported this line after a clean 0.13.9 delivery,
    // and cycling it would have been tidiness: the running binary had no
    // functional symptom, and restarting a load-bearing resolver to silence a
    // diff is how a degraded host becomes a down host. So the line now states
    // the condition under which the restart is actually required, and leaves
    // the judgement where it belongs.
    for row in rows
        .iter()
        .filter(|row| row.process_cell() == PROCESS_DIFFERS)
    {
        eprintln!(
            "{}: the process under {} is running {} — not the artefact this \
             unit's declaration resolves to. A stale process is not itself a \
             fault. Restart it when the running binary lacks behaviour you now \
             need — it cannot parse a registry value a newer version added, or \
             a fix that this process executes has been delivered — and not \
             merely because this line is printed.",
            row.binary,
            row.unit,
            row.running_binary.as_deref().unwrap_or(UNKNOWN)
        );
    }
    for entry in pass.releases.iter().filter(|entry| entry.status == FAILED) {
        eprintln!("{} {}: {}", entry.binary, entry.version, entry.detail);
    }
    for entry in &pass.undeliverable {
        eprintln!("{}: {}", entry.binary, entry.detail);
    }
    Ok(())
}
