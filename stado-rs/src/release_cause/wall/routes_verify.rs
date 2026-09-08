//! Reading `skarbiec route verify` — the one predicate this fleet has.

use super::WallVerdict;
use crate::release_cause::classify::bound;

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
    Some(bound(&format!("{resource}: {problem}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `skarbiec routes verify <resource>` with a route whose item is absent.
    /// Captured verbatim from running the command; the report goes to stdout
    /// AND the command exits non-zero.
    const VERIFY_BROKEN: &str = r#"{
  "broken": [
    {
      "problem": "no vault item absent-item",
      "resource": "provider:kimi:brama-sub-wisent-app-kimi-primary"
    }
  ],
  "checked": 1
}"#;

    /// Captured verbatim: a scope that matched no route at all. Exit ZERO.
    const VERIFY_CHECKED_NONE: &str = r#"{
  "broken": [],
  "checked": 0
}"#;

    const VERIFY_CLEAN: &str = r#"{"broken": [], "checked": 7}"#;

    #[test]
    fn a_named_broken_route_is_the_wall_standing() {
        assert_eq!(
            read_routes_verify(false, VERIFY_BROKEN),
            WallVerdict::Present
        );
        assert_eq!(
            routes_verify_detail(VERIFY_BROKEN).as_deref(),
            Some("provider:kimi:brama-sub-wisent-app-kimi-primary: no vault item absent-item")
        );
    }

    #[test]
    fn every_route_resolving_is_the_wall_gone() {
        assert_eq!(read_routes_verify(true, VERIFY_CLEAN), WallVerdict::Gone);
    }

    #[test]
    fn a_scope_that_checked_nothing_is_not_permission_to_promote() {
        // Captured from the real command: a resource absent from the routes
        // table exits ZERO with `checked: 0`. Trusting the exit status would
        // turn a vanished route into a green light.
        assert_eq!(
            read_routes_verify(true, VERIFY_CHECKED_NONE),
            WallVerdict::Unknown
        );
    }

    #[test]
    fn a_check_that_could_not_run_is_never_a_finding_either_way() {
        // Every one of these exits non-zero with nothing on stdout: no routes
        // table, a vault that will not open, a missing binary, a timeout.
        // None of them may read as the wall being gone, and none of them may
        // be reported as the wall standing.
        for stdout in [
            "",
            "   ",
            "Error: no capability routes table at /x/y.json",
            "{",
        ] {
            assert_eq!(
                read_routes_verify(false, stdout),
                WallVerdict::Unknown,
                "misread {stdout:?}"
            );
        }
        // Exit zero with an unreadable report is equally no answer.
        assert_eq!(read_routes_verify(true, "not json"), WallVerdict::Unknown);
    }
}
