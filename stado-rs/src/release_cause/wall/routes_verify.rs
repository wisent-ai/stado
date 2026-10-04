//! Reading `skarbiec route verify` — the one predicate this fleet has.

use super::WallVerdict;
use crate::release_cause::classify::evidence_line;

/// Read `skarbiec route verify`'s answer, exit status and report together.
///
/// The contract was confirmed by running the command, not inferred from its
/// name, and it needs both halves because a bare exit status cannot carry it:
///
/// - broken routes → exit non-zero AND the report on stdout with a non-empty
///   `broken` array. This is the only shape that means the wall is standing.
/// - no routes table, or a vault that will not open → exit non-zero with
///   **empty stdout**. Indistinguishable from the above by status alone, which
///   is why the report is parsed rather than the code trusted.
/// - every route resolves → exit zero, `broken` empty, `checked` at least one.
/// - **a scope that matched no route → exit zero, `checked:
///   0`.** Reading exit zero as "gone" would turn a resource that vanished
///   from the routes table into permission to promote, which is the failure
///   this whole predicate exists to prevent. `checked:
///   0` is [`WallVerdict::Unknown`].
pub fn read_routes_verify(success: bool, stdout: &str) -> WallVerdict {
    let report: Option<serde_json::Value> = serde_json::from_str(stdout.trim()).ok();
    let broken = report
        .as_ref()
        .and_then(|report| report.get("broken"))
        .and_then(serde_json::Value::as_array);
    let checked = report
        .as_ref()
        .and_then(|report| report.get("checked"))
        .and_then(serde_json::Value::as_u64);
    if !success {
        // A refusal that names the routes it refused is the wall. A non-zero
        // exit with nothing to show for it is a broken check, not a finding.
        return match broken {
            Some(broken) if !broken.is_empty() => WallVerdict::Present,
            _ => WallVerdict::Unknown,
        };
    }
    match (checked, broken) {
        (Some(0), _) | (None, _) => WallVerdict::Unknown,
        (Some(_), Some(broken)) if !broken.is_empty() => WallVerdict::Present,
        (Some(_), _) => WallVerdict::Gone,
    }
}

/// The sentence a refusal quotes for what the predicate saw.
///
/// The vault's own words for the first broken route, so the operator reads the
/// same problem `skarbiec doctor` would show them rather than a paraphrase.
pub fn routes_verify_detail(stdout: &str) -> Option<String> {
    let report: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let first = report.get("broken")?.as_array()?.first()?;
    let resource = first.get("resource")?.as_str()?;
    let problem = first.get("problem")?.as_str()?;
    Some(evidence_line(&format!("{resource}: {problem}")))
}
