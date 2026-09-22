//! What a refusal record carries, and what it still parses from.

use super::super::*;
use super::{decide, state_with, RUN};
use crate::release_agent::rollout::recover::run::REPEAT_CAUSE_LIMIT;
use crate::release_agent::state::evidence::clip_middle;
use crate::release_cause::QuarantineCause;

/// Retention: the clip used to keep the head and drop the end, and both
/// ends carry decisive lines in the live records.
#[test]
fn a_clipped_tail_keeps_both_of_its_ends() {
    let text = format!("HEAD-MARKER{}TAIL-MARKER", "x".repeat(4000));
    let clipped = clip_middle(&text, 200);
    assert!(clipped.starts_with("HEAD-MARKER"), "{clipped}");
    assert!(clipped.ends_with("TAIL-MARKER"), "{clipped}");
    assert!(
        clipped.contains("elided"),
        "an elision must say how much it dropped: {clipped}"
    );
    assert_eq!(clip_middle("short", 200), "short");
}

#[test]
fn a_quarantine_record_names_its_cause_from_its_reason() {
    let record = QuarantineRecord::new(
        "release 0.2.54 does not declare rollback compatibility with 0.2.53".to_string(),
    );
    assert_eq!(
        record.cause,
        QuarantineCause::RollbackCompatibilityUndeclared
    );
    assert!(!record.evidence.is_empty());
}

/// The twenty records already on the live host carry neither field, and
/// this struct refuses unknown fields. Both directions have to work.
#[test]
fn a_record_written_before_this_change_still_parses() {
    let legacy = r#"{"reason":"candidate did not become ready before deadline",
            "quarantined_at":"2026-08-06T15:49:52.887004+00:00"}"#;
    let record: QuarantineRecord =
        serde_json::from_str(legacy).expect("a legacy record must still parse");
    assert_eq!(record.cause, QuarantineCause::Unclassified);
    assert!(record.evidence.is_empty());
}

/// Three probes the host could not answer are three statements about the
/// host. Counting them held charless-mac-mini on a hand-built Brama while
/// every released candidate was refused before it could try.
#[test]
fn a_run_of_probes_the_host_never_answered_does_not_hold_the_next_candidate() {
    let starved: Vec<(&str, QuarantineCause, &str)> = RUN
        .iter()
        .map(|(digest, _, stamp)| (*digest, QuarantineCause::ReadinessProbeUnanswered, *stamp))
        .collect();
    assert_eq!(
        starved.len(),
        REPEAT_CAUSE_LIMIT,
        "the run must reach the limit"
    );
    assert!(
        decide(&state_with(&starved), None).is_none(),
        "a host that could not answer walled off the release"
    );
    // The same run of a cause about the release still holds, so this is a
    // statement about the cause and not a hole in the rule.
    assert!(
        decide(&state_with(RUN), None).is_some(),
        "a repeated credential wall must still hold"
    );
}
