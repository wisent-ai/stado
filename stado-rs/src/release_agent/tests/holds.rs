//! What holds the next candidate back, and what lets it go.

use super::super::*;
use super::{decide, state_with, GONE, PRESENT, RUN};
use crate::release_agent::rollout::recover::run::REPEAT_CAUSE_LIMIT;
use crate::release_cause::{self, QuarantineCause};

#[test]
fn an_observed_wall_holds_at_the_very_first_quarantine() {
    // This is the whole point of the change. One row plus a condition that
    // still reports the wall is sufficient; the old rule spent two more
    // candidates establishing what the check already said.
    let hold = decide(&state_with(&RUN[..1]), PRESENT)
        .expect("one quarantine and an observed wall must hold");
    assert!(matches!(hold.ground, HoldGround::Observed { .. }));
    let sentence = hold.sentence();
    assert!(
        sentence.contains("still refuses") && sentence.contains("Checked with"),
        "an observed hold must say what it observed and when: {sentence}"
    );
}

#[test]
fn a_repaired_credential_releases_promotion_with_no_override() {
    // The property counting cannot have. The operator refills the vault
    // field, the check stops failing, and the next candidate goes -- no
    // `quarantine clear`, nothing to remember.
    assert!(decide(&state_with(&RUN[..1]), GONE).is_none());
    // Including past the counting limit: an observation beats a tally.
    assert!(decide(&state_with(RUN), GONE).is_none());
}

#[test]
fn an_unreachable_check_never_releases_a_hold_and_never_invents_one() {
    let unknown = |why: &str| Some((release_cause::WallVerdict::Unknown, Some(why.to_string())));
    // Not permission to promote: the count still governs, exactly as before
    // the predicate existed.
    let hold = decide(&state_with(RUN), unknown("no skarbiec binary at /x"))
        .expect("an unreachable check must fall back to counting, not release");
    match &hold.ground {
        HoldGround::Repeated { unreachable, .. } => assert_eq!(
            unreachable.as_deref(),
            Some("no skarbiec binary at /x"),
            "the refusal must admit the check could not be reached"
        ),
        other => panic!("expected a counted hold, got {other:?}"),
    }
    assert!(
        hold.sentence().contains("could not be checked"),
        "the ground must not read as an observation: {}",
        hold.sentence()
    );
    // And it does not manufacture a refusal on a short run either.
    assert!(decide(&state_with(&RUN[..1]), unknown("timeout")).is_none());
}

#[test]
fn counting_still_governs_a_cause_with_no_condition_to_ask() {
    // N=3 unchanged where it applies. `verdict: None` is what `cause_hold`
    // does when the cause has no predicate.
    let rows: Vec<(&str, QuarantineCause, &str)> = RUN
        .iter()
        .map(|(digest, _, stamp)| {
            (
                *digest,
                QuarantineCause::CapabilityRedemptionRefused,
                *stamp,
            )
        })
        .collect();
    assert!(decide(&state_with(&rows[..2]), None).is_none());
    let hold = decide(&state_with(&rows), None).expect("three of one cause still holds");
    assert!(matches!(hold.ground, HoldGround::Repeated { .. }));
}

#[test]
fn a_run_of_unnamed_causes_never_holds_anything() {
    // Twelve of the twenty live records are unclassified, seven of them
    // consecutively. Refusing on a cause the agent could not name would
    // have frozen this product for a month on no evidence at all.
    let unnamed: Vec<(&str, QuarantineCause, &str)> = RUN
        .iter()
        .map(|(digest, _, stamp)| (*digest, QuarantineCause::Unclassified, *stamp))
        .collect();
    assert!(cause_run(&state_with(&unnamed)).is_none());
    assert!(decide(&state_with(&unnamed), PRESENT).is_none());
}

#[test]
fn one_different_cause_below_the_top_shortens_the_run() {
    // The live shape: b54ea076 credential_cannot_serve sits above
    // aba3c3b2 rollback_compatibility_undeclared, so the run is ONE.
    // Counting alone would not hold, which is why the condition matters.
    let mut mixed = RUN.to_vec();
    mixed[1].1 = QuarantineCause::RollbackCompatibilityUndeclared;
    let run = cause_run(&state_with(&mixed)).expect("the newest row still names a cause");
    assert_eq!(run.cause, QuarantineCause::CredentialCannotServe);
    assert_eq!(run.len(), 1);
    assert!(!run.repeats());
    // Counting lets it burn; the observed wall does not.
    assert!(decide(&state_with(&mixed), None).is_none());
    assert!(decide(&state_with(&mixed), PRESENT).is_some());
}

#[test]
fn recency_is_read_from_the_stamp_not_from_the_digest_order() {
    // The map is keyed by digest, so its iteration order is alphabetical.
    // A rule that trusted that order would pick the wrong rows.
    let mut rows = RUN.to_vec();
    rows.push((
        "0000aaaa",
        QuarantineCause::RollbackCompatibilityUndeclared,
        "2026-08-06T15:49:52Z",
    ));
    let run = cause_run(&state_with(&rows))
        .expect("the oldest row sorts first by digest and must not join the run");
    assert_eq!(run.cause, QuarantineCause::CredentialCannotServe);
    assert_eq!(run.len(), REPEAT_CAUSE_LIMIT);
    assert!(!run.digests.iter().any(|digest| digest == "0000aaaa"));
    // The oldest member of the run, not of the map.
    assert_eq!(run.since.to_rfc3339(), "2026-09-01T10:34:46+00:00");
}

#[test]
fn every_refusal_names_a_way_out() {
    for verdict in [PRESENT, None] {
        let sentence = decide(&state_with(RUN), verdict)
            .expect("a run of three holds either way")
            .sentence();
        assert!(
            sentence.contains("stado release quarantine clear --digest 217167ef"),
            "refusal must name the existing override and a real digest: {sentence}"
        );
        assert!(
            sentence.contains("skarbiec route verify"),
            "refusal must carry the cause's remedy: {sentence}"
        );
    }
}

/// Retention: the clip used to keep the head and drop the end, and both
