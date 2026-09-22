//! The decisions this agent makes about a refusal, exercised from the names
//! the rest of the crate reads them by.

use chrono::{DateTime, Utc};

use super::*;
use crate::release_agent::rollout::recover::run::REPEAT_CAUSE_LIMIT;
use crate::release_agent::state::evidence::clip_middle;
use crate::release_cause::{self, QuarantineCause};

fn state_with(quarantines: &[(&str, QuarantineCause, &str)]) -> HostReleaseState {
    let mut state = HostReleaseState::new("brama", "charless-mac-mini");
    for (index, (digest, cause, stamp)) in quarantines.iter().enumerate() {
        state.quarantined.insert(
            (*digest).to_string(),
            QuarantineRecord {
                reason: format!("candidate did not become ready within 90s: reason {index}"),
                quarantined_at: DateTime::parse_from_rfc3339(stamp)
                    .expect("fixture stamp parses")
                    .with_timezone(&Utc),
                cause: *cause,
                evidence: format!("evidence {index}"),
            },
        );
    }
    state
}

/// The digests and stamps are the live `brama` rows from
/// `charless-mac-mini` for 0.2.49, 0.2.50 and 0.2.51 — the three candidates
/// burned inside five hours on 2026-09-01, at the interval this rule is
/// meant for.
///
/// The cause is supplied. On the real host those three wrote no failure
/// line anywhere in their logs and classify as `unclassified`, so no
/// refusal arms there and none should. The run is constructed because the
/// rule has to be exercised somewhere, and inventing the timestamps too
/// would have hidden that the real sequence is this tight.
const RUN: &[(&str, QuarantineCause, &str)] = &[
    (
        "217167ef",
        QuarantineCause::CredentialCannotServe,
        "2026-09-01T10:34:46Z",
    ),
    (
        "d862fb1b",
        QuarantineCause::CredentialCannotServe,
        "2026-09-01T10:54:23Z",
    ),
    (
        "4c2bb7c3",
        QuarantineCause::CredentialCannotServe,
        "2026-09-01T15:40:50Z",
    ),
];

/// The product's own decision, with the verdict supplied instead of spawned.
///
/// The decision and the spawn are separated in
/// [`crate::release_agent::rollout::recover::wall::hold_for`] so the decision
/// can be exercised for every verdict, including the ones a real host will
/// not produce on demand — a vault that will not open, a timeout. This calls
/// that function rather than repeating it, so the two cannot drift.
fn decide(
    state: &HostReleaseState,
    verdict: Option<(release_cause::WallVerdict, Option<String>)>,
) -> Option<CauseHold> {
    crate::release_agent::rollout::recover::wall::hold_for(cause_run(state)?, verdict)
}

const PRESENT: Option<(release_cause::WallVerdict, Option<String>)> =
    Some((release_cause::WallVerdict::Present, None));
const GONE: Option<(release_cause::WallVerdict, Option<String>)> =
    Some((release_cause::WallVerdict::Gone, None));

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

/// A candidate that could not bind used to be filed as `unclassified`: the
/// only trace of the collision was the operating system's own sentence
/// inside a stderr tail. The agent's reason for lukasz-macbook's Skarbiec
/// 0.3.10, verbatim.
#[test]
fn a_candidate_that_could_not_bind_names_the_occupied_port() {
    let reason = "candidate did not become ready within 90s: pid 44850 is gone; stderr \
                  /Users/lukaszbartoszcze/.stado/logs/skarbiec-0.3.10.err: skarbiec API \
                  listening on http://127.0.0.1:18788 (loopback only) | Error: bind \
                  127.0.0.1:18787 |  | Caused by: |     Address already in use (os error 48); \
                  stdout [is empty]";
    let classified = release_cause::classify(reason);
    assert_eq!(classified.cause, QuarantineCause::StableBindOccupied);
    assert!(
        classified.cause.holds_the_candidate(),
        "a port another program holds is a wall the next candidate meets too"
    );
    assert!(
        classified
            .cause
            .remedy()
            .is_some_and(|remedy| remedy.contains("stado service serving")),
        "the remedy must name the command that reports the holder"
    );
}

/// The guard that makes the record above unnecessary: ask the kernel who
/// holds the bind before spending ninety seconds on a candidate that cannot
/// take it.
#[test]
fn a_stable_bind_another_process_holds_is_named_before_a_candidate_is_spawned() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let bind = listener
        .local_addr()
        .expect("the bound address")
        .to_string();
    let target = crate::release_control::ReleaseTargetPolicy {
        platform: "darwin-arm64".to_string(),
        run_as_user: whoami(),
        home: "/nonexistent".to_string(),
        state_dir: "/nonexistent".to_string(),
        runtime_root: "/nonexistent".to_string(),
        logs_root: "/nonexistent".to_string(),
        stable_bind: Some(bind.clone()),
        candidate_ports: Some([0, 1]),
        readiness_path: Some("/readyz".to_string()),
        legacy_launchd_label: None,
        legacy_launchd_plist: None,
    };
    let serving = target.blue_green_serving().expect("blue-green coordinates");
    let holder = crate::release_agent::rollout::serving::discover::foreign_stable_bind_holder(
        &target, &serving, "skarbiec",
    )
    .expect("the reader answers");
    if crate::release_agent::rollout::serving::discover::lsof_binary().is_none() {
        // Documented: a host that cannot tell answers unknown, and an unknown
        // never refuses a rollout.
        assert!(holder.is_none(), "no lsof, no verdict: {holder:?}");
        return;
    }
    let holder = holder.expect("a held port must be reported");
    assert!(
        holder.contains(&bind) && holder.contains(&std::process::id().to_string()),
        "the sentence must name the bind and the pid holding it: {holder}"
    );
}

/// The same reader must not invent a holder for a port nobody took, or every
/// rollout would refuse itself.
#[test]
fn a_free_stable_bind_reports_no_holder() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.local_addr().expect("the bound address").port()
    };
    let target = crate::release_control::ReleaseTargetPolicy {
        platform: "darwin-arm64".to_string(),
        run_as_user: whoami(),
        home: "/nonexistent".to_string(),
        state_dir: "/nonexistent".to_string(),
        runtime_root: "/nonexistent".to_string(),
        logs_root: "/nonexistent".to_string(),
        stable_bind: Some(format!("127.0.0.1:{port}")),
        candidate_ports: Some([0, 1]),
        readiness_path: Some("/readyz".to_string()),
        legacy_launchd_label: None,
        legacy_launchd_plist: None,
    };
    let serving = target.blue_green_serving().expect("blue-green coordinates");
    assert!(
        crate::release_agent::rollout::serving::discover::foreign_stable_bind_holder(
            &target, &serving, "skarbiec",
        )
        .expect("the reader answers")
        .is_none(),
        "a released port has no holder to name"
    );
}

/// The account the release runs as. Only its presence matters here: the
/// reader asks the kernel about a port, never about this field.
fn whoami() -> String {
    std::env::var("USER").unwrap_or_else(|_| "operator".to_string())
}

/// The check that decided the fleet's credential plane was dead. Every Mac
/// here runs Stado through `~/.local/bin/stado`, a symlink to
/// `~/.stado/bin/stado`, and `ps -o comm=` on this system answers with the
/// bare program name. Comparing that against a full path made a process fail
/// to recognise itself, and `credentials item show` refused every read with
/// `recorded stable proxy pid N does not match the exact executable and
/// arguments` while the proxy it refused was the one the agent had started.
#[test]
fn a_running_process_recognises_its_own_executable() {
    let me = i32::try_from(std::process::id()).expect("a pid fits");
    let executable = std::env::current_exe().expect("this test has an executable");
    assert!(
        crate::release_agent::rollout::serving::discover::process_executable_matches(
            me,
            &executable
        ),
        "a process must match the executable it is running"
    );
}

/// Two names for one file are one executable. A release proxy started through
/// the symlink on `PATH` and a check that resolved the installed path are the
/// same program.
#[test]
fn one_executable_reached_by_two_names_is_one_executable() {
    use crate::release_agent::rollout::serving::discover::same_executable;
    // This package's own build directory: a unit test is not handed
    // `CARGO_TARGET_TMPDIR`, and a fixture under the operator's home would
    // depend on their machine instead of on this package.
    let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/tmp"));
    std::fs::create_dir_all(&root).expect("target tmp");
    let dir = tempfile::TempDir::new_in(&root).expect("a directory");
    let installed = dir.path().join("stado");
    std::fs::write(&installed, b"#!/bin/sh\nexit 0\n").expect("the installed file");
    let linked = dir.path().join("stado-on-path");
    std::os::unix::fs::symlink(&installed, &linked).expect("the symlink every Mac here has");
    assert!(
        same_executable(&linked, &installed),
        "a symlink and its target are one program"
    );
    let other = dir.path().join("skarbiec");
    std::fs::write(&other, b"#!/bin/sh\nexit 0\n").expect("a second file");
    assert!(
        !same_executable(&other, &installed),
        "two different files are not one program"
    );
}

/// The stable bind can be held by two very different things, and the
/// refusal has to tell them apart. On charless-mac-mini on 2026-09-21 the
/// sentence read like a stray process to kill; what held the port was
/// `com.wisent.always-on.skarbiec`, the fleet's own managed unit serving
/// the bind directly, which is a host still in the pre-proxy shape.
#[test]
fn the_stable_bind_refusal_tells_a_foreign_holder_from_the_product_itself() {
    use crate::release_agent::rollout::serving::discover::describe_holder;

    let itself = describe_holder("127.0.0.1:8895", 44394, "skarbiec", "skarbiec");
    assert!(
        itself.contains("served directly by skarbiec itself"),
        "the product on its own bind is the pre-proxy shape: {itself}"
    );
    assert!(
        itself.contains("move off the stable bind"),
        "and the sentence names what has to change: {itself}"
    );

    let foreign = describe_holder("127.0.0.1:8895", 501, "python3", "skarbiec");
    assert!(
        foreign.contains("is not skarbiec's release proxy"),
        "another program on the port stays a collision: {foreign}"
    );
    assert!(
        !foreign.contains("served directly"),
        "and is never described as the product itself: {foreign}"
    );
}

/// The loop charless-mac-mini was in on 2026-09-21, as a rule.
///
/// The declared unit held 8895, so the agent recorded `no candidate was
/// spawned` and never spawned one; stopping that unit made the bind-repair
/// pass put it straight back — `restored legacy skarbiec on 127.0.0.1:8895` —
/// and the next tick read the same held bind. Behind that loop: no credential
/// write on the host, `weles-api` dead at boot, no account signed in.
#[test]
fn a_candidate_that_never_held_the_bind_is_owed_it_before_the_declared_unit() {
    use crate::release_agent::tick::product::candidate_is_owed_the_bind;

    let document: serde_json::Value =
        serde_json::from_str(include_str!("../../data/release-policies/skarbiec.json"))
            .expect("the shipped skarbiec policy parses");
    let mut policy: crate::release_control::ProductReleasePolicy =
        serde_json::from_value(document["policy"].clone()).expect("the policy document is current");
    policy.desired = Some(
        serde_json::from_value(serde_json::json!({
            "version": "0.3.12",
            "channel": "stable",
            "rollout_generation": 13,
            "promoted_at": "2026-09-21T06:24:31Z",
            "artifacts": {
                "darwin-arm64": {
                    "manifest_uri": "https://example.invalid/manifest.json",
                    "signature_uri": "https://example.invalid/manifest.sig",
                    "archive_uri": "https://example.invalid/skarbiec.tar.zst",
                    "artifact_sha256": DESIRED_DIGEST,
                    "manifest_sha256": "b".repeat(64),
                    "source_revision": "bba611a37572818a0e1db7c31ea95e506efd176b",
                    "key_id": "wisent-release-2026"
                }
            }
        }))
        .expect("the desired release document is current"),
    );
    let target = policy.targets["charless-mac-mini"].clone();

    let mut held = HostReleaseState::new("skarbiec", "charless-mac-mini");
    held.rollout_generation = 13;
    held.phase = RolloutPhase::Failed;
    held.detail = format!(
        "127.0.0.1:8895 is held by pid 40304 (skarbiec), which is not skarbiec's release proxy; {}",
        crate::release_agent::NO_CANDIDATE_SPAWNED
    );
    assert!(
        candidate_is_owed_the_bind(&held, &policy, &target),
        "a release that never got the bind has to be given this tick, or the unit takes it back"
    );

    // The net this exception is carved out of: with the digest quarantined
    // there is nothing to roll out, and the bind belongs to the declared unit
    // — the state that once left this host serving no Skarbiec for thirteen
    // hours.
    let mut quarantined = held.clone();
    quarantined.quarantined.insert(
        DESIRED_DIGEST.to_string(),
        QuarantineRecord::new("candidate did not become ready within 90s".to_string()),
    );
    assert!(
        !candidate_is_owed_the_bind(&quarantined, &policy, &target),
        "with nothing to roll out the declared unit keeps the bind"
    );

    // A settled rollout is not owed anything either: its own proxy holds the
    // bind and this pass must not take it away.
    let mut settled = HostReleaseState::new("skarbiec", "charless-mac-mini");
    settled.rollout_generation = 13;
    settled.phase = RolloutPhase::Committed;
    settled.detail = "release committed after rollback window".to_string();
    assert!(
        !candidate_is_owed_the_bind(&settled, &policy, &target),
        "a committed release is not a candidate waiting for the bind"
    );
}

/// The digest the fixture above calls desired, in the shape a manifest uses.
const DESIRED_DIGEST: &str = "a3f61691a3f61691a3f61691a3f61691a3f61691a3f61691a3f61691a3f61691";
