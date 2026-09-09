//! Which fleet builder may claim a queued release build.
//!
//! The subject is the worker's own explicit admission decision: an
//! `accepting_jobs: false` publication is authoritative and carries the same
//! blockers as `stado host gates`, while a publication that says nothing —
//! what a worker running an older build writes during a rolling upgrade — is
//! unknown rather than a refusal.
//!
//! Every case drives `stado release submit`, which is the command an operator
//! runs to get a product built, and asserts what the product wrote back into
//! its store: the immutable build request naming the builder that was allowed
//! to claim, the queued job pinned to that host, or the release-run document
//! carrying the exact refusal and no build at all.
//!
//! The refusal sentences below are copied from live runs of the built binary
//! against this machine; only the platform and the declared host name are
//! substituted, because those are this machine's own.

mod document;
mod fixture;

use serde_json::{json, Value};

use document::{consumer, platform, TARGET};
use fixture::{stderr, Fixture};

/// The exit status a refused submission leaves. One, not a signal, because an
/// operator's shell and every wrapper around it read that.
const REFUSED: i32 = 1;

/// The refusal `stado release submit` prints and persists when no live host
/// may claim the build, with `verdict` the words the selector had for the one
/// declared host.
fn refusal(verdict: &str) -> String {
    format!(
        "no live fleet builder can CLAIM release_platform {}; capacity read from the configured \
         queue store namespace \"builder-claim\" listed 1 live consumer(s) and the registry \
         declares 1 target(s) for that platform. Considered: {TARGET} {verdict}. A host that \
         publishes capacity but claims nothing cannot build: read `stado host gates <host>` for \
         the full verdict.",
        platform()
    )
}

/// The run document of a submission that never reached a builder: failed, with
/// the refusal as its recorded cause and not one platform recorded.
fn assert_refused(fixture: &Fixture, verdict: &str) {
    let run = fixture.run_document();
    assert_eq!(run["state"], Value::String("failed".into()), "run: {run}");
    assert_eq!(
        run["failure"],
        Value::String(refusal(verdict)),
        "the release run must record the claim refusal itself"
    );
    assert_eq!(
        run["platforms"],
        json!({}),
        "a refused platform must not be recorded as a submitted build: {run}"
    );
    assert!(
        fixture.build_request().is_none(),
        "no build request may be written for a host that cannot claim"
    );
    assert!(
        fixture.queued_jobs().is_empty(),
        "no job may be queued for a host that cannot claim"
    );
}

/// The claim record of a submission that did reach a builder: the immutable
/// request names the host, and the job is pinned to the consumer that host
/// publishes as.
fn assert_claimed(fixture: &Fixture) {
    let mut submit = fixture.spawn_submit();
    let (request, job) = fixture.claim(&mut submit);
    assert_eq!(
        request["builder"],
        Value::String(TARGET.into()),
        "the build request must name the builder allowed to claim: {request}"
    );
    assert_eq!(
        request["platform"],
        Value::String(platform().into()),
        "request: {request}"
    );
    assert_eq!(
        job["pinned_host"],
        Value::String(consumer()),
        "the queued build must be pinned to the claiming consumer: {job}"
    );
    assert_eq!(job["state"], Value::String("queued".into()), "job: {job}");
}

/// A host that published a refusal is given no build, and the run says which
/// host refused and in whose words.
#[test]
fn a_host_that_refuses_work_receives_no_build() {
    let fixture = Fixture::new();
    fixture.publish(
        Some(json!(false)),
        json!({"admission_reason": "ram_exhausted"}),
    );

    let output = fixture.submit();
    assert_eq!(output.status.code(), Some(REFUSED), "{}", stderr(&output));
    assert_refused(&fixture, "not accepting jobs; reasons: ram_exhausted");
    assert!(
        stderr(&output).contains(&refusal("not accepting jobs; reasons: ram_exhausted")),
        "the operator is told the same sentence the run recorded: {}",
        stderr(&output)
    );
}

/// An old inferred slot table does not overrule an explicit refusal: the
/// worker said no, and free slots are not a second opinion.
#[test]
fn free_slots_do_not_overrule_an_explicit_refusal() {
    let fixture = Fixture::new();
    fixture.publish(
        Some(json!(false)),
        json!({"admission_reason": "ram_exhausted"}),
    );
    let capacity = fixture
        .store()
        .join("capacity")
        .join(format!("{}.json", consumer()));
    let mut publication: Value =
        serde_json::from_slice(&std::fs::read(&capacity).expect("read the publication"))
            .expect("the publication is JSON");
    publication["free_slots"] = json!({"cpu": 99});
    std::fs::write(
        &capacity,
        serde_json::to_vec_pretty(&publication).expect("the publication serialises"),
    )
    .expect("rewrite the publication");

    let output = fixture.submit();
    assert_eq!(output.status.code(), Some(REFUSED), "{}", stderr(&output));
    assert_refused(&fixture, "not accepting jobs; reasons: ram_exhausted");
}

/// A refusal carries the host-gate words the worker published, so the sentence
/// an operator reads here and the one `stado host gates` prints are one
/// vocabulary.
#[test]
fn a_refusal_carries_the_host_gate_words_the_worker_published() {
    let fixture = Fixture::new();
    fixture.publish(
        Some(json!(false)),
        json!({
            "disk_pressure_active": true,
            "disk_pressure_unresolved": true,
            "disk_cleanup_policy_known": false,
            "queue_paused": true,
            "disk_cleanup": {"outcome": "lock_busy", "lock_busy": true},
        }),
    );

    let output = fixture.submit();
    assert_eq!(output.status.code(), Some(REFUSED), "{}", stderr(&output));
    assert_refused(
        &fixture,
        "not accepting jobs; reasons: disk_pressure_active (release deliveries only), \
         disk_pressure_unresolved, disk_cleanup_policy_unknown, queue_paused, \
         disk_cleanup_stalled (janitor pass lock_busy)",
    );
}

/// A refusal with no published reason is still a refusal, and the run says so
/// rather than inventing a cause.
#[test]
fn a_refusal_without_a_published_reason_stays_a_refusal() {
    let fixture = Fixture::new();
    fixture.publish(Some(json!(false)), json!({}));

    let output = fixture.submit();
    assert_eq!(output.status.code(), Some(REFUSED), "{}", stderr(&output));
    assert_refused(&fixture, "not accepting jobs; no reason published");
}

/// A host that accepts work receives the build, pinned to the consumer it
/// publishes as.
#[test]
fn an_accepting_host_receives_the_build_pinned_to_it() {
    let fixture = Fixture::new();
    fixture.publish(Some(json!(true)), json!({}));
    assert_claimed(&fixture);
}

/// A publication from before the admission decision existed is unknown, not a
/// refusal: a rolling upgrade must not stop the fleet from building.
#[test]
fn a_publication_with_no_admission_decision_still_receives_the_build() {
    let fixture = Fixture::new();
    fixture.publish(None, json!({"storage_backend": "stado"}));
    assert_claimed(&fixture);
}

/// An admission value that is not a boolean is unknown too. The selector must
/// not read a truthy shape as consent.
#[test]
fn a_non_boolean_admission_value_is_not_read_as_a_refusal() {
    let fixture = Fixture::new();
    fixture.publish(Some(json!("yes")), json!({}));
    assert_claimed(&fixture);
}

/// A host that refuses only because it is busy right now still receives the
/// queued build: the claim gate on the host waits for the resources, and
/// treating "busy" like a disk or policy gate would leave the build unbuilt
/// while the fleet was merely working.
#[test]
fn a_host_that_is_only_busy_still_receives_the_queued_build() {
    let fixture = Fixture::new();
    fixture.publish(Some(json!(false)), json!({"admission_reason": "cpu_busy"}));
    assert_claimed(&fixture);
}
