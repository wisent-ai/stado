//! Who holds the stable bind, and whether a candidate is owed it.

use super::super::*;

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
