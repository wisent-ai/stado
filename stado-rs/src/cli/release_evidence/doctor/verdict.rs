//! The rule itself: every fact the verdict is computed from, and the one
//! verdict computed over them.

use serde_json::{json, Value};

use crate::release_agent::CauseRun;

use super::super::constants::{
    BLOCKER_CANDIDATE_NOT_READY, BLOCKER_DESIRED_DIGEST_QUARANTINED, BLOCKER_REPEATING_CAUSE,
    BLOCKER_STABLE_BIND_HELD, HEALTH_NO_CANDIDATE, HEALTH_OK, HEALTH_UNPROBED,
    REMEDY_DESIRED_DIGEST_QUARANTINED, REMEDY_STABLE_BIND_HELD, VERDICT_BLOCKED, VERDICT_ROLLING,
    VERDICT_SETTLED,
};
use super::super::quarantine::cause_summary;

/// Every fact the verdict is computed from, so the rule itself holds no I/O
/// and can be exercised against the exact state the incident left behind.
pub(super) struct Facts<'a> {
    pub(super) product: &'a str,
    pub(super) target: &'a str,
    pub(super) desired_version: Option<&'a str>,
    pub(super) observed_version: Option<&'a str>,
    pub(super) phase: &'a str,
    pub(super) detail: &'a str,
    pub(super) candidate: Value,
    pub(super) quarantined: Vec<Value>,
    pub(super) gates: Value,
    /// The blockers the host's own queue agent published, in its words.
    pub(super) gate_blockers: Vec<String>,
    pub(super) disk_pressure_unresolved: bool,
    /// The most recent quarantines sharing one named cause, as the agent's own
    /// detector reports them.
    ///
    /// This command cannot ask the cause's condition: the check reads the
    /// release user's vault ON the host, and running it here would resolve this
    /// operator's own store instead. So the run is reported, the count is
    /// resolved into a certain hold where counting alone decides, and the check
    /// the agent will run is named so an operator can run it themselves.
    pub(super) run: Option<CauseRun>,
    /// A candidate is recorded, or the phase says one is being staged.
    pub(super) in_flight: bool,
}

/// The verdict and the report around it.
///
/// `blocked` is the only verdict that says the rollout will not move on its
/// own, and it now has four causes: the agent refuses a quarantined desired
/// digest on every pass, a host with an unresolved disk gate fails admission
/// closed and claims nothing at all, the agent refuses to spend another
/// candidate on a cause the last few all failed for, and another declaration
/// holds the stable bind so no candidate is ever spawned. Everything short of
/// converged is `rolling`, because the agent's next tick is what advances it.
pub(super) fn diagnosis(facts: &Facts<'_>) -> Value {
    let mut blockers = facts.gate_blockers.clone();
    let desired_quarantined = facts
        .quarantined
        .iter()
        .any(|entry| entry["is_desired_digest"] == Value::Bool(true));
    if desired_quarantined {
        blockers.push(BLOCKER_DESIRED_DIGEST_QUARANTINED.to_string());
    }
    // A candidate that answers nothing is listed even while the verdict stays
    // `rolling`: the rollout is still inside its readiness window, and the
    // next thing that happens to it is the quarantine this command exists to
    // explain.
    if facts.candidate["health_status"]
        .as_str()
        .is_some_and(|status| {
            status != HEALTH_OK && status != HEALTH_NO_CANDIDATE && status != HEALTH_UNPROBED
        })
    {
        blockers.push(BLOCKER_CANDIDATE_NOT_READY.to_string());
    }
    // Only a hold that counting alone decides is asserted as a blocker. A
    // shorter run whose cause has a checkable condition is reported as pending,
    // not claimed: this command cannot reach the condition, and asserting a
    // refusal it did not establish would be the same overreach as naming a
    // cause from a symptom.
    let held = facts.run.as_ref().is_some_and(CauseRun::repeats);
    if held {
        blockers.push(BLOCKER_REPEATING_CAUSE.to_string());
    }
    // The agent writes this when a foreign program holds the stable bind, and
    // then waits for a declaration that has no reason to yield. Every pass
    // repeats it, so it is a stop with a decision behind it, not a rollout in
    // flight.
    let stable_bind_held = facts.detail.contains(crate::release_agent::NO_CANDIDATE_SPAWNED);
    if stable_bind_held {
        blockers.push(BLOCKER_STABLE_BIND_HELD.to_string());
    }
    blockers.sort();
    blockers.dedup();
    let converged =
        facts.observed_version.is_some() && facts.observed_version == facts.desired_version;
    let verdict = if desired_quarantined
        || facts.disk_pressure_unresolved
        || held
        || stable_bind_held
    {
        VERDICT_BLOCKED
    } else if facts.in_flight || !converged {
        VERDICT_ROLLING
    } else {
        VERDICT_SETTLED
    };
    let summary = cause_summary(&facts.quarantined);
    // Remedies are for what is blocking *now*, not for every cause in the
    // host's history. A settled product with an old quarantine was printing
    // "declare rollback compatibility, then promote it again" under a verdict
    // of `settled`, which reads as work to do where there is none. The
    // historical repairs stay in the per-cause table, where they are framed as
    // history.
    let mut remedies = Vec::new();
    if desired_quarantined {
        // The cause first, the clear second, in that order and for that
        // reason: clearing the digest without repairing what it failed on just
        // spends another candidate on the same wall. This is the mistake the
        // incident repeated.
        if let Some(remedy) = facts
            .quarantined
            .iter()
            .find(|entry| entry["is_desired_digest"] == Value::Bool(true))
            .and_then(|entry| entry["remedy"].as_str())
        {
            remedies.push(remedy.to_string());
        }
        remedies.push(REMEDY_DESIRED_DIGEST_QUARANTINED.to_string());
    }
    if let Some(remedy) = facts
        .run
        .as_ref()
        .filter(|_| held)
        .and_then(|run| run.cause.remedy())
    {
        remedies.push(remedy.to_string());
    }
    if stable_bind_held {
        remedies.push(REMEDY_STABLE_BIND_HELD.to_string());
    }
    remedies.dedup();
    json!({
        "product": facts.product,
        "target": facts.target,
        "desired_version": facts.desired_version,
        "observed_version": facts.observed_version,
        "phase": facts.phase,
        "detail": facts.detail,
        "candidate": facts.candidate,
        "quarantine_summary": summary,
        "cause_run": facts.run.as_ref().map(|run| {
            let predicate = run.cause.predicate(&run.evidence);
            json!({
                "cause": run.cause.as_str(),
                "quarantines": run.len(),
                "since": run.since.to_rfc3339(),
                "evidence": run.evidence,
                "digests": run.digests,
                // `held` is what this command established. `condition_check`
                // is what the agent will ask before spending a candidate, and
                // is the same command an operator can run by hand.
                "held": held,
                "hold_ground": if held { "repeated" } else { "pending_condition_check" },
                "condition_check": predicate
                    .as_ref()
                    .map(|check| format!("skarbiec {}", check.args.join(" "))),
                "condition_resource": predicate.as_ref().map(|check| check.resource.clone()),
            })
        }),
        "quarantined": facts.quarantined,
        "gates": facts.gates,
        "verdict": verdict,
        "blockers": blockers,
        "remedies": remedies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The facts `release doctor` read off charless-mac-mini on 2026-09-21,
    /// with only the detail line differing between the two cases.
    fn facts<'a>(detail: &'a str) -> Facts<'a> {
        Facts {
            product: "skarbiec",
            target: "charless-mac-mini",
            desired_version: Some("0.3.12"),
            observed_version: None,
            phase: "failed",
            detail,
            candidate: json!({ "health_status": HEALTH_NO_CANDIDATE }),
            quarantined: Vec::new(),
            gates: json!({}),
            gate_blockers: Vec::new(),
            disk_pressure_unresolved: false,
            run: None,
            in_flight: false,
        }
    }

    #[test]
    fn a_stable_bind_another_declaration_holds_is_blocked_and_says_how_it_ends() {
        let report = diagnosis(&facts(
            "127.0.0.1:8895 is held by pid 40304 (skarbiec), which is not skarbiec's release \
             proxy; no candidate was spawned",
        ));
        assert_eq!(report["verdict"], VERDICT_BLOCKED);
        assert!(
            report["blockers"]
                .as_array()
                .expect("blockers is an array")
                .contains(&Value::from(BLOCKER_STABLE_BIND_HELD)),
            "the held bind is not named as a blocker: {report}"
        );
        let remedies = report["remedies"]
            .as_array()
            .expect("remedies is an array")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            remedies.contains("stado service list") && remedies.contains("retire"),
            "the remedy does not say how the standoff ends: {remedies}"
        );
    }

    /// The same unconverged host with an ordinary detail is still rolling:
    /// the agent's next tick advances it, and calling that blocked would send
    /// an operator to retire a unit nothing is waiting on.
    #[test]
    fn a_rollout_that_is_merely_unfinished_is_not_blocked() {
        let report = diagnosis(&facts("fetching skarbiec 0.3.12"));
        assert_eq!(report["verdict"], VERDICT_ROLLING);
        assert!(
            report["blockers"]
                .as_array()
                .expect("blockers is an array")
                .is_empty(),
            "an unfinished rollout reported blockers: {report}"
        );
    }
}
