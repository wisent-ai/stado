//! What the agent retires by itself, and what it writes down when it does.

use chrono::Duration;
use stado::release_agent::{
    last_auto_retirement, retire_host_caused_quarantine, RetireVerdict, AGENT_ACTOR,
    AUTO_RETIRE_COOLDOWN_SECONDS,
};

use crate::fixture::{StateDir, DIGEST, PROBE_REASON, PRODUCT, TARGET};

/// The defect: a probe the host never answered kept the desired digest refused
/// on every pass. Retiring it must change the document, not just the report.
#[test]
fn a_probe_the_host_never_answered_is_retired_by_the_agent() {
    let dir = StateDir::new();
    let mut state = dir.state(&[(DIGEST, PROBE_REASON)]);

    let verdict = retire_host_caused_quarantine(dir.as_str(), TARGET, PRODUCT, DIGEST, &mut state)
        .expect("the record exists");

    assert!(
        matches!(verdict, RetireVerdict::Retire(_)),
        "a host-caused refusal must be retired: {verdict:?}"
    );
    assert!(
        !state.quarantined.contains_key(DIGEST),
        "the retired digest must leave the rollout state"
    );
}

/// Retiring is a rewrite of the host's own refusal, so it is accountable in
/// the same file an operator's `quarantine clear` writes — with the evidence
/// the rewrite deletes from the state document.
#[test]
fn the_agents_retirement_is_written_in_the_operators_audit_trail() {
    let dir = StateDir::new();
    let mut state = dir.state(&[(DIGEST, PROBE_REASON)]);

    retire_host_caused_quarantine(dir.as_str(), TARGET, PRODUCT, DIGEST, &mut state)
        .expect("the record exists");

    let entries = dir.audit_entries();
    assert_eq!(entries.len(), 1, "one retirement, one line: {entries:?}");
    let entry = &entries[0];
    assert_eq!(entry["actor"], AGENT_ACTOR);
    assert_eq!(entry["digest"], DIGEST);
    assert_eq!(entry["product"], PRODUCT);
    assert_eq!(entry["host"], TARGET);
    assert_eq!(entry["cause"], "readiness_probe_unanswered");
    assert_eq!(
        entry["quarantine_reason"], PROBE_REASON,
        "the account must keep the evidence the state document lost"
    );
    assert!(
        last_auto_retirement(dir.as_str(), PRODUCT, DIGEST).is_some(),
        "the agent must be able to read its own retirement back"
    );
}

/// The bound. Retiring is a retry, and a host that still cannot run the
/// release quarantines the digest again within the readiness window: without
/// the wait, that pair is one burned candidate per tick.
#[test]
fn a_second_retirement_inside_the_cooldown_is_refused() {
    let dir = StateDir::new();
    dir.seed_retirement(DIGEST, Duration::minutes(1));
    let mut state = dir.state(&[(DIGEST, PROBE_REASON)]);

    let verdict = retire_host_caused_quarantine(dir.as_str(), TARGET, PRODUCT, DIGEST, &mut state)
        .expect("the record exists");

    match verdict {
        RetireVerdict::Cooling { seconds_left, .. } => assert!(
            seconds_left > 0 && seconds_left <= AUTO_RETIRE_COOLDOWN_SECONDS,
            "the wait must be the remaining cooldown, got {seconds_left}"
        ),
        other => panic!("a retirement one minute old must still be cooling: {other:?}"),
    }
    assert!(
        state.quarantined.contains_key(DIGEST),
        "a refused retirement must leave the refusal in place"
    );
    assert_eq!(
        dir.audit_entries().len(),
        1,
        "a refused retirement writes nothing"
    );
}

/// The recovery this exists for: an hour later the host may have changed, and
/// nothing has to remember that but the trail itself.
#[test]
fn a_retirement_older_than_the_cooldown_lets_the_agent_try_again() {
    let dir = StateDir::new();
    dir.seed_retirement(
        DIGEST,
        Duration::seconds(AUTO_RETIRE_COOLDOWN_SECONDS) + Duration::seconds(1),
    );
    let mut state = dir.state(&[(DIGEST, PROBE_REASON)]);

    let verdict = retire_host_caused_quarantine(dir.as_str(), TARGET, PRODUCT, DIGEST, &mut state)
        .expect("the record exists");

    assert!(
        matches!(verdict, RetireVerdict::Retire(_)),
        "past the cooldown the agent tries again: {verdict:?}"
    );
    assert!(!state.quarantined.contains_key(DIGEST));
    assert_eq!(
        dir.audit_entries().len(),
        2,
        "the second retirement is its own line"
    );
}
