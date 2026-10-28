//! `stado service ensure` as the command prints it: the pass's receipt as
//! JSON or as one table row, with the audit record it left.

use super::*;

/// Idempotently assert one service from the declared program and the host's
/// observed unit. The same receipt serves the CLI and internal callers.
pub(crate) async fn ensure(options: EnsureOptions<'_>) -> Result<(), CmdError> {
    let as_json = options.as_json;
    let receipt = ensure_unit(options).await?;
    if as_json {
        // The per-unit results are part of the JSON contract as well as stderr.
        if let Some(audited) = receipt.audited.as_deref() {
            eprintln!("audit record {audited}");
        }
        return print_json(&json!({
            "host": receipt.host,
            "name": receipt.name,
            "label": receipt.label,
            "domain": receipt.domain,
            "action": receipt.action,
            "pid": receipt.pid,
            "predecessors": receipt.predecessors,
        }));
    }
    let pid = match receipt.pid {
        Some(pid) => pid.to_string(),
        None => crate::cli::reporting::NOT_REPORTED.to_string(),
    };
    table::print(
        &["HOST", "SERVICE", "LABEL", "DOMAIN", "ACTION", "PID"],
        &[vec![
            receipt.host,
            receipt.name,
            receipt.label,
            receipt.domain,
            receipt.action,
            pid,
        ]],
    );
    if let Some(audited) = receipt.audited.as_deref() {
        println!("audit record {audited}");
    }
    Ok(())
}
