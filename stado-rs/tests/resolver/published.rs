//! What a resolver that has published its state says about itself.

use serde_json::json;

use crate::fixture::{wait_published, Policy};
use crate::readiness::{serving, status, HELD_GENERATION, ROLLED_BACK_GENERATION};
use crate::{report, said, Host};

#[test]
fn a_directory_that_rolls_backwards_is_refused_and_the_reason_reaches_the_operator() {
    let policy = Policy::eager(HELD_GENERATION);
    let host = Host::new(&policy.document());
    let (mut resolver, _) = serving(&host, &policy);

    // The authority's document goes backwards under a live resolver. The
    // refresh refuses it rather than serving an older directory, and the
    // reason is published instead of living only in this process's stderr.
    let mut rolled = policy.document();
    rolled["service_directory"]["generation"] = json!(ROLLED_BACK_GENERATION);
    host.write_registry(&rolled);

    let published = wait_published(&host, "backing_off");
    assert_eq!(
        published["reason"],
        format!(
            "service directory rollback rejected: generation {ROLLED_BACK_GENERATION} < \
             {HELD_GENERATION}"
        )
    );
    assert!(
        published["attempt"].as_u64().unwrap_or_default() >= 1,
        "a backing-off resolver counted no attempt: {published}"
    );
    assert!(
        published["next_attempt_at"].is_string(),
        "a backing-off resolver scheduled no next read: {published}"
    );

    let answer = status(&host);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    let degraded = report(&answer);
    assert_eq!(degraded["state"], "backing_off");
    assert_eq!(degraded["reason"], published["reason"]);
    let blocker = degraded["blockers"][0].as_str().unwrap_or_default();
    assert!(
        blocker.starts_with(&format!(
            "the resolver reports state backing_off: service directory rollback rejected: \
             generation {ROLLED_BACK_GENERATION} < {HELD_GENERATION} (failed attempt "
        )),
        "got: {blocker}"
    );
    assert!(
        blocker.contains(", next read due "),
        "the blocker does not say when the next read is due: {blocker}"
    );
    assert_eq!(
        degraded["api"]["listening"], true,
        "a resolver that cannot refresh still serves what it holds: {degraded}"
    );
    assert!(
        resolver.running(),
        "the resolver died on a refusal it is supposed to retry: {}",
        resolver.said()
    );
}
