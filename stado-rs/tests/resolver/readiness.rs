//! What a real resolver on this machine reports about itself.
//!
//! Every state under test is produced by a real `resolver serve` process
//! against the isolated local registry: it binds the loopback ports its own
//! target declares, publishes its own state file, and holds the generation it
//! loaded. Nothing here writes that state file by hand, and no answer comes
//! from a remote destination — the authority read is the local store read,
//! which each report states as `authority.source == "local"`.

use std::time::Instant;

use serde_json::{json, Value};

use crate::fixture::{wait_listening, wait_published, Policy, Serving, BUDGET};
use crate::{report, said, stdout, Host, CONSUMER, SERVICE, TARGET};

/// The generation the fixture's authority starts at, and the one it advances
/// to while a resolver keeps the first — both are declared data, and the
/// blocker names both numbers rather than the word "stale".
const HELD_GENERATION: u64 = 7;
const ADVANCED_GENERATION: u64 = 9;
/// One generation below what a live resolver holds: the value that makes the
/// next refresh a rollback the resolver must refuse.
const ROLLED_BACK_GENERATION: u64 = 6;

fn status(host: &Host) -> std::process::Output {
    host.stado(&["resolver", "status", "--target", TARGET, "--json"])
}

/// Bring a real resolver up and prove it is the one serving: both declared
/// binds accepted, and the published state names this process.
fn serving(host: &Host, policy: &Policy) -> (Serving, Value) {
    let mut resolver = Serving::start(host, &["resolver", "serve", "--target", TARGET]);
    assert!(
        wait_listening(policy.api),
        "the resolver never bound its declared API 127.0.0.1:{}: {}",
        policy.api,
        resolver.said()
    );
    assert!(
        wait_listening(policy.adapter),
        "the resolver never bound its declared adapter 127.0.0.1:{}: {}",
        policy.adapter,
        resolver.said()
    );
    let published = wait_published(host, "serving");
    assert_eq!(
        published["pid"].as_u64(),
        Some(u64::from(resolver.pid())),
        "another process published this host's resolver state: {published}"
    );
    assert!(
        resolver.running(),
        "the resolver exited: {}",
        resolver.said()
    );
    (resolver, published)
}

#[test]
fn a_serving_resolver_is_ready_until_the_authority_moves_past_the_generation_it_holds() {
    let policy = Policy::patient(HELD_GENERATION);
    let host = Host::new(&policy.document());
    let (mut resolver, published) = serving(&host, &policy);
    assert_eq!(published["generation"], HELD_GENERATION);

    let answer = status(&host);
    assert!(
        answer.status.success(),
        "a ready resolver exited non-zero: {}",
        said(&answer)
    );
    let ready = report(&answer);
    assert_eq!(ready["verdict"], "ready");
    assert_eq!(ready["state"], "serving");
    assert_eq!(ready["generation"], HELD_GENERATION);
    assert_eq!(ready["stale"], false);
    assert_eq!(ready["blockers"], json!([]));
    assert_eq!(ready["api"]["listening"], true);
    assert_eq!(ready["adapters"][0]["listening"], true);
    assert_eq!(ready["authority"]["source"], "local");
    assert_eq!(ready["authority"]["reachable"], true);
    assert_eq!(ready["authority"]["generation"], HELD_GENERATION);
    assert_eq!(
        ready["bind_probe"],
        format!("probed: these binds are loopback addresses on {TARGET}, which is this host")
    );
    assert_eq!(
        ready["registry_staleness_seconds"],
        Value::Null,
        "a fresh authority read reports no registry staleness: {ready}"
    );

    // The authority publishes a newer directory. This resolver will not read
    // it again inside the case, so it is genuinely behind.
    let mut advanced = policy.document();
    advanced["service_directory"]["generation"] = json!(ADVANCED_GENERATION);
    host.write_registry(&advanced);

    let answer = status(&host);
    assert_eq!(
        answer.status.code(),
        Some(1),
        "a resolver behind the authority must exit 1: {}",
        said(&answer)
    );
    let behind = report(&answer);
    assert_eq!(behind["verdict"], "degraded");
    assert_eq!(behind["stale"], true);
    assert_eq!(behind["generation"], HELD_GENERATION);
    assert_eq!(behind["authority"]["generation"], ADVANCED_GENERATION);
    assert_eq!(
        behind["blockers"],
        json!([format!(
            "the resolver holds service directory generation {HELD_GENERATION} and the authority \
             publishes {ADVANCED_GENERATION}"
        )])
    );
    assert!(
        resolver.running(),
        "the resolver died while it was merely behind: {}",
        resolver.said()
    );
}

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
