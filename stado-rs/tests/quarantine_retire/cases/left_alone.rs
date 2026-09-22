//! What the agent leaves exactly as it found it, and what it refuses by name.

use stado::release_agent::{retire_host_caused_quarantine, RetireVerdict};

use crate::fixture::{StateDir, DIGEST, PRODUCT, TARGET, VAULT_REASON};

/// The other half of the contract. A refusal that names the candidate is the
/// operator's to clear, and the agent must not touch it — not the document,
/// not the trail.
#[test]
fn a_refusal_that_names_the_candidate_is_left_for_the_operator() {
    let dir = StateDir::new();
    let mut state = dir.state(&[(DIGEST, VAULT_REASON)]);

    let verdict = retire_host_caused_quarantine(dir.as_str(), TARGET, PRODUCT, DIGEST, &mut state)
        .expect("the record exists");

    match &verdict {
        RetireVerdict::CandidateHeld(cause) => assert_eq!(
            cause.as_str(),
            "credential_store_unreadable",
            "the cause is read from the record's own evidence"
        ),
        other => panic!("a vault that will not open holds the candidate: {other:?}"),
    }
    assert!(
        state.quarantined.contains_key(DIGEST),
        "the operator's refusal must survive the agent's pass"
    );
    assert!(
        dir.audit_entries().is_empty(),
        "nothing was retired, so nothing is recorded"
    );
    assert!(
        verdict.detail().contains("stado release quarantine clear"),
        "the state detail must name the command that does clear it: {}",
        verdict.detail()
    );
}

/// A digest nobody quarantined is a caller error, not a silent success: the
/// tick asks only about the digest it just found in the map.
#[test]
fn a_digest_that_is_not_quarantined_is_refused_by_name() {
    let dir = StateDir::new();
    let mut state = dir.state(&[]);

    let error = retire_host_caused_quarantine(dir.as_str(), TARGET, PRODUCT, DIGEST, &mut state)
        .expect_err("there is no such record");

    assert!(
        error.contains(DIGEST) && error.contains(PRODUCT),
        "the refusal must name what was asked for: {error}"
    );
}
