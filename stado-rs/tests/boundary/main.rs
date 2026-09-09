//! Authorization-boundary recovery on the object plane.
//!
//! On 2026-08-19 `com.wisent.always-on.stado-object-api` — which is
//! `stado dashboard --bind 127.0.0.1 --port 8765` — answered
//! `503 {"error":"object authorization unavailable"}` to the whole fleet
//! because one bad vault read at startup shut the `object` boundary and
//! nothing ever revalidated it. Clearing it needed a privileged LaunchDaemon
//! restart, which is exactly what the product's own recovery path cannot do.
//!
//! Every case here runs that command as its own process against a real
//! Skarbiec broker, and asserts what the listener served and what it
//! published about itself: the exact 503 body, the verifier's own sentence in
//! `last_error`, the cooldown, recovery without a restart — proved by the
//! process id the listener publishes staying the same across it — and the
//! release boundary reopening through a route that never gates on it.

mod fixture;
mod frozen;
mod policy;
mod vault;

use serde_json::Value;

use fixture::{past_cooldown, Env, OBJECT_UNAVAILABLE};
use policy::{ABSENT_KEY, NAMESPACE, UNDECLARED_RELEASE_KEY};
use vault::{bearer, object_item, Vault};

/// How the verifier's own words reach `last_error` when the refusal is about
/// this host's deployment rather than about the vault being unreachable.
/// Copied from a live run of the listener, prefix and spacing included.
const DEPLOYMENT: &str = "Skarbiec deployment configuration: ";

/// One object read, addressed the way an operator's client addresses it.
fn object_target(namespace: &str, key: &str) -> String {
    format!("/api/object?uri=stado://{namespace}/{key}")
}

/// A shut object boundary reopens by itself once its grant is repaired, and
/// the fleet gets its object plane back inside the running process.
#[test]
fn a_shut_object_boundary_reopens_without_a_restart() {
    let env = Env::new();
    let items = policy::object_items();
    let vault = Vault::start(&env.home(), &items);
    // The grant the fleet had: one namespace's item was never added to it, so
    // the verifier's item set does not match the policy and the boundary is
    // shut the moment the process boots.
    let (withheld, granted) = items.split_last().expect("the policy names items");
    vault.grant(
        stado::config::OBJECT_API_VERIFIER_CONSUMER,
        granted,
        &env.grant("object"),
    );
    let listener = env.start(&vault.url(), &vault.url());
    let target = object_target(NAMESPACE, ABSENT_KEY);
    let namespace_bearer = bearer(&object_item(NAMESPACE));

    // The boot sweep is itself a revalidation attempt, and it anchors the
    // cooldown, so the window is let pass before the first read: what this
    // case is about is a request revalidating a shut boundary, not the boot
    // verdict answering for it.
    past_cooldown();
    let refused = listener.get(&target, Some(&namespace_bearer));
    assert_eq!(refused.status, 503, "body: {}", refused.body);
    assert_eq!(refused.body, OBJECT_UNAVAILABLE);

    // The reason is the verifier's own sentence, published where an operator
    // already has the read permission.
    let closed = listener.boundary("object");
    assert_eq!(closed["ready"], Value::Bool(false), "{closed}");
    assert_eq!(
        closed["last_error"],
        Value::String(format!(
            "{DEPLOYMENT}object verifier grant item set mismatch \
             (missing=[{withheld}], unexpected=[])"
        )),
        "the operator must read what refused and about which item"
    );
    assert!(
        closed["checked_at"].is_string(),
        "a closed verdict is timestamped: {closed}"
    );
    let pid = listener.pid();

    // Liveness answers before authorization, so it stays flat booleans: no
    // vault item, grant or endpoint leaks to an unauthenticated prober.
    let health = listener.get("/healthz", None);
    assert_eq!(health.status, 200, "body: {}", health.body);
    let health = health.json();
    assert_eq!(health["ok"], Value::Bool(true));
    assert_eq!(health["degraded"], Value::Bool(true));
    assert_eq!(
        health["boundaries"]["object"],
        Value::Bool(false),
        "liveness publishes the verdict as a bare boolean: {health}"
    );
    assert!(
        !health.to_string().contains("last_error"),
        "no boundary reason on the unauthenticated liveness route: {health}"
    );

    // The grant is repaired in place — the file every verifier re-reads per
    // attempt — and nothing is restarted, reloaded or signalled. The
    // revalidation the read above claimed is the cooldown's anchor, so the
    // burst below is inside the window by construction.
    let anchor = listener.boundary("object")["checked_at"].clone();
    vault.grant(
        stado::config::OBJECT_API_VERIFIER_CONSUMER,
        &items,
        &env.grant("object"),
    );

    // A burst inside the cooldown is answered from the recorded verdict, so a
    // fleet hammering a shut boundary cannot turn into one vault sweep per
    // request: the repaired grant is not read yet.
    for _ in 0..5 {
        let repeated = listener.get(&target, Some(&namespace_bearer));
        assert_eq!(repeated.status, 503, "body: {}", repeated.body);
        assert_eq!(repeated.body, OBJECT_UNAVAILABLE);
    }
    assert_eq!(
        listener.boundary("object")["checked_at"],
        anchor,
        "the cooldown held: the burst revalidated nothing"
    );

    // Past the cooldown the next request revalidates inline and passes the
    // boundary, authorizes against the namespace bearer, and reads the store.
    past_cooldown();
    let recovered = listener.get(&target, Some(&namespace_bearer));
    assert_eq!(
        recovered.status, 404,
        "the request passed the boundary and read the store: {}",
        recovered.body
    );
    assert_eq!(
        recovered.body,
        format!(r#"{{"state":"absent","uri":"stado://{NAMESPACE}/{ABSENT_KEY}"}}"#)
    );

    let open = listener.boundary("object");
    assert_eq!(open["ready"], Value::Bool(true), "{open}");
    assert_eq!(open["last_error"], Value::Null);
    assert_ne!(
        open["checked_at"], anchor,
        "the recovered verdict carries its own timestamp"
    );
    assert_eq!(
        listener.pid(),
        pid,
        "recovery happened inside the process that was already serving"
    );
}

/// No boundary may be its own precondition for reopening.
///
/// `Boundary::Release` was in that state twice: first required by no route at
/// all, then "fixed" by requiring it on the release-coordinate object routes —
/// which are exactly the routes excluded from the boundary check because the
/// key is a release key. So it was closed once and closed for the life of the
/// process, and no request, credential or amount of asking could reopen it.
///
/// The repair separates asking from enforcing, and this case drives both
/// halves against the running listener: a release-coordinate read is refused
/// for its key and not for the shut boundary, and that same read is what gives
/// the boundary its way back.
#[test]
fn a_release_coordinate_reopens_a_boundary_it_is_not_gated_by() {
    let env = Env::new();
    let object = policy::object_items();
    let publishers = policy::publisher_items();
    let mut items = object.clone();
    items.extend(publishers.iter().cloned());
    let vault = Vault::start(&env.home(), &items);
    // The object boundary is whole, because a release-coordinate request
    // spends its one revalidation on the first closed boundary in its plan and
    // `object` comes first.
    vault.grant(
        stado::config::OBJECT_API_VERIFIER_CONSUMER,
        &object,
        &env.grant("object"),
    );
    let (withheld, granted) = publishers
        .split_last()
        .expect("the policy names release publishers");
    vault.grant(
        stado::config::RELEASE_API_VERIFIER_CONSUMER,
        granted,
        &env.grant("release"),
    );
    let listener = env.start(&vault.url(), &vault.url());

    let closed = listener.boundary("release");
    assert_eq!(
        closed["ready"],
        Value::Bool(false),
        "the release boundary starts shut in this case: {closed}"
    );
    assert_eq!(
        closed["last_error"],
        Value::String(format!(
            "{DEPLOYMENT}release verifier grant item set mismatch \
             (missing=[{withheld}], unexpected=[])"
        ))
    );
    assert_eq!(
        listener.boundary("object")["ready"],
        Value::Bool(true),
        "the object boundary must be open for this case to be about release"
    );

    // Asking is not enforcing: the request is refused for its key, by the
    // release authorization it reached, and not by the shut boundary. The
    // boot sweep anchored the cooldown, so the window is let pass first.
    past_cooldown();
    let target = object_target("releases", UNDECLARED_RELEASE_KEY);
    let refused = listener.get(&target, None);
    assert_eq!(
        refused.status, 401,
        "a shut release boundary must not gate this route: {}",
        refused.body
    );
    assert_eq!(
        refused.json()["reason"],
        Value::String("no_publisher_for_key".into()),
        "body: {}",
        refused.body
    );
    let asked = listener.boundary("release");
    assert_ne!(
        asked["checked_at"], closed["checked_at"],
        "the request the boundary does not gate is the request that revalidates it"
    );

    // And it is a way back, not only a fresh timestamp: with the grant
    // repaired, the same ungated route reopens the boundary.
    vault.grant(
        stado::config::RELEASE_API_VERIFIER_CONSUMER,
        &publishers,
        &env.grant("release"),
    );
    past_cooldown();
    let reopening = listener.get(&target, None);
    assert_eq!(reopening.status, 401, "body: {}", reopening.body);
    let open = listener.boundary("release");
    assert_eq!(
        open["ready"],
        Value::Bool(true),
        "the release boundary reopened through a route that never gated on it: {open}\n{}",
        listener.logged()
    );
    assert_eq!(open["last_error"], Value::Null);
}
