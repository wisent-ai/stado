//! A refusal that named the host must not outlive the host's condition.
//!
//! # The invariant
//!
//! `QuarantineCause::holds_the_candidate` divides refusals into statements
//! about the release and statements about the host. A host that could not
//! answer a three-second readiness probe said nothing about the bytes it
//! failed to start, so the agent retires that record by itself and rolls the
//! desired digest out again. A refusal that names the candidate — a vault that
//! will not open, an undeclared rollback compatibility — is still cleared only
//! by `stado release quarantine clear`, on the operator's audit line.
//!
//! # What went wrong
//!
//! On `lukasz-macbook` the desired Skarbiec digest
//! `55f2cf470e293d03c920ee1b4184e5144c98acbc7fe6315771be892b1b9791b4` was
//! quarantined at 2026-09-17T21:50:31Z with `active release lost readiness:
//! http://127.0.0.1:18788/readyz did not answer within 3s`. The agent's tick
//! read the map, set `phase: quarantined`, and returned — on that pass and
//! every pass after it. Three days later `release doctor` still reported
//! `observed -`, and every command resolving the release-controlled Skarbiec
//! binary refused with `no observed active release (phase Quarantined)`:
//! credential reads, grant reads, `release catalog declare-publisher`, and
//! with them the Most provider credential and the fleet's release publication.
//!
//! # What is defended here
//!
//! The persisted effects, not a log line: the record leaves the state
//! document, this agent's own retirement is appended to the same audit trail
//! the operator command writes, a second retirement inside the cooldown is
//! refused so a host that still cannot run the release does not spend one
//! candidate per tick, a retirement older than the cooldown is allowed again,
//! and a candidate-naming cause is left exactly as it was found.

use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use serde_json::{json, Value};
use stado::release_agent::{
    last_auto_retirement, parse_state_document, quarantine_audit_path,
    retire_host_caused_quarantine, HostReleaseState, RetireVerdict, AGENT_ACTOR,
    AUTO_RETIRE_COOLDOWN_SECONDS, STATE_SCHEMA,
};

const PRODUCT: &str = "skarbiec";
const TARGET: &str = "lukasz-macbook";
/// The live digest the incident left quarantined on this machine.
const DIGEST: &str = "55f2cf470e293d03c920ee1b4184e5144c98acbc7fe6315771be892b1b9791b4";
/// The agent's own sentence for that record, copied from the host.
const PROBE_REASON: &str = "active release lost readiness: http://127.0.0.1:18788/readyz did not \
                            answer within 3s; stderr \
                            /Users/lukaszbartoszcze/.stado/logs/skarbiec-0.3.10.err: skarbiec API \
                            listening on http://127.0.0.1:18788 (loopback only)";
/// A refusal that names the candidate: the vault could not be opened at all.
const VAULT_REASON: &str = "candidate did not become ready within 90s: \
                            http://127.0.0.1:18895/readyz answered HTTP 503 Service Unavailable; \
                            stderr skarbiec readiness monitor: stored item cannot be decrypted: \
                            spawn gpg: No such file or directory (os error 2)";
/// The stamp both live records carry, kept so the audit assertions compare
/// against the host's own value rather than a second one invented here.
const QUARANTINED_AT: &str = "2026-09-17T21:50:31.922394Z";
/// The rollout generation of that host's Skarbiec declaration. Any generation
/// exercises the same branch; this is the one the incident ran under.
const GENERATION: u64 = 7;

/// A state directory of the shape the agent keeps, inside this package's own
/// build directory rather than the operating system's shared temporary one.
struct StateDir {
    dir: tempfile::TempDir,
}

impl StateDir {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(&root).expect("target tmp");
        Self {
            dir: tempfile::TempDir::new_in(root).expect("state dir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn as_str(&self) -> &str {
        self.path().to_str().expect("utf-8 state dir")
    }

    /// One rollout state document carrying exactly the quarantines given, read
    /// back through the agent's own parser so the fixture cannot describe a
    /// document the agent would refuse.
    fn state(&self, quarantines: &[(&str, &str)]) -> HostReleaseState {
        let mut map = serde_json::Map::new();
        for (digest, reason) in quarantines {
            map.insert(
                (*digest).to_string(),
                json!({ "reason": reason, "quarantined_at": QUARANTINED_AT }),
            );
        }
        let document = json!({
            "schema_version": STATE_SCHEMA,
            "product": PRODUCT,
            "target": TARGET,
            "rollout_generation": GENERATION,
            "phase": "quarantined",
            "quarantined": Value::Object(map),
            "detail": "desired release digest is quarantined on this host",
            "updated_at": QUARANTINED_AT,
        });
        parse_state_document(
            &serde_json::to_vec(&document).expect("document"),
            PRODUCT,
            TARGET,
            "fixture",
        )
        .expect("the fixture must be a document the agent accepts")
    }

    fn audit_entries(&self) -> Vec<Value> {
        let path = quarantine_audit_path(self.as_str(), PRODUCT);
        match std::fs::read_to_string(path) {
            Ok(payload) => payload
                .lines()
                .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Write one retirement of `digest` as if it had happened `age` ago.
    fn seed_retirement(&self, digest: &str, age: Duration) {
        let line = json!({
            "actor": AGENT_ACTOR,
            "host": TARGET,
            "product": PRODUCT,
            "digest": digest,
            "reason": "seeded",
            "cause": "readiness_probe_unanswered",
            "audited_at": (Utc::now() - age).to_rfc3339(),
            "quarantine_reason": PROBE_REASON,
            "quarantined_at": QUARANTINED_AT,
        });
        let path = quarantine_audit_path(self.as_str(), PRODUCT);
        std::fs::write(path, format!("{line}\n")).expect("seed the trail");
    }
}

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
