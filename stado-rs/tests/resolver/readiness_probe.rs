//! What the readiness probe says about a resolver that never ran, one that
//! was killed while serving, and a directory whose generation rolls
//! backwards.

use std::time::Instant;

use serde_json::{json, Value};

use crate::fixture::{wait_published, Policy, BUDGET};
use crate::readiness::{serving, status, HELD_GENERATION, ROLLED_BACK_GENERATION};
use crate::{report, said, stdout, Host, CONSUMER, SERVICE, TARGET};

#[test]
fn a_resolver_that_has_never_run_is_reported_down_with_the_file_and_the_ports_named() {
    let policy = Policy::patient(HELD_GENERATION);
    let host = Host::new(&policy.document());

    let answer = status(&host);
    assert_eq!(
        answer.status.code(),
        Some(1),
        "a resolver that is not running must exit 1: {}",
        said(&answer)
    );
    let down = report(&answer);
    assert_eq!(down["verdict"], "down");
    assert_eq!(down["state"], "unpublished");
    assert_eq!(down["generation"], Value::Null);
    assert_eq!(
        down["stale"], true,
        "holding no generation is not freshness: {down}"
    );
    assert_eq!(down["api"]["listening"], false);
    assert_eq!(down["adapters"][0]["listening"], false);
    assert_eq!(
        down["blockers"],
        json!([
            format!(
                "no resolver has published state at {}: nothing has served here since that file \
                 was last removed",
                host.state_path().display()
            ),
            format!(
                "nothing is listening on the resolution API at 127.0.0.1:{}",
                policy.api
            ),
            format!(
                "nothing is listening on the {SERVICE} adapter for consumer {CONSUMER} at \
                 127.0.0.1:{}",
                policy.adapter
            ),
        ])
    );
    assert!(
        !host.state_path().exists(),
        "status is a read: it must not create the file it reports missing"
    );

    // The same facts one per line, for an operator reading a terminal.
    let answer = host.stado(&["resolver", "status", "--target", TARGET]);
    assert_eq!(answer.status.code(), Some(1));
    let text = stdout(&answer);
    assert!(
        text.starts_with(&format!(
            "resolver {TARGET} state=unpublished verdict=down generation=- stale=yes\n"
        )),
        "got: {text}"
    );
    assert!(
        text.contains(&format!("api 127.0.0.1:{} not-listening\n", policy.api)),
        "got: {text}"
    );
    assert!(
        text.contains(&format!(
            "authority {TARGET} source=local reachable generation={HELD_GENERATION}\n"
        )),
        "got: {text}"
    );
}

#[test]
fn a_resolver_killed_while_serving_goes_stale_by_the_window_its_target_declares() {
    let policy = Policy::eager(HELD_GENERATION);
    let host = Host::new(&policy.document());
    let (mut resolver, _) = serving(&host, &policy);

    // Killed outright, so nothing publishes a last word: the state file keeps
    // saying `serving` while the ports it named are gone. That is the shape
    // the launchd restart loop left behind, and the readiness answer has to
    // age the snapshot itself rather than take the state's own word.
    resolver.end();

    let deadline = Instant::now() + BUDGET;
    let mut aged = report(&status(&host));
    while Instant::now() < deadline
        && aged["generation_age_seconds"].as_i64().unwrap_or_default()
            <= policy.max_stale_seconds as i64
    {
        aged = report(&status(&host));
    }

    let age = aged["generation_age_seconds"].as_i64().unwrap_or_default();
    assert_eq!(aged["state"], "serving", "{aged}");
    assert_eq!(aged["generation"], HELD_GENERATION);
    assert_eq!(aged["max_stale_seconds"], policy.max_stale_seconds);
    assert_eq!(aged["stale"], true);
    assert_eq!(aged["api"]["listening"], false);
    assert_eq!(aged["adapters"][0]["listening"], false);
    assert_eq!(
        aged["blockers"],
        json!([
            format!(
                "nothing is listening on the resolution API at 127.0.0.1:{}",
                policy.api
            ),
            format!(
                "nothing is listening on the {SERVICE} adapter for consumer {CONSUMER} at \
                 127.0.0.1:{}",
                policy.adapter
            ),
            format!(
                "the snapshot the resolver holds is {age}s old, past the {}s max-stale window \
                 this target declares",
                policy.max_stale_seconds
            ),
        ])
    );
    assert_eq!(
        aged["verdict"], "degraded",
        "a resolver that published a snapshot is not `down`: {aged}"
    );
}

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
