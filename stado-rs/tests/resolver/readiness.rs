//! What a real resolver on this machine reports about itself.
//!
//! Every state under test is produced by a real `resolver serve` process
//! against the isolated local registry: it binds the loopback ports its own
//! target declares, publishes its own state file, and holds the generation it
//! loaded. Nothing here writes that state file by hand, and no answer comes
//! from a remote destination — the authority read is the local store read,
//! which each report states as `authority.source == "local"`.


use serde_json::{json, Value};

use crate::fixture::{wait_listening, wait_published, Policy, Serving};
use crate::{report, said, Host, TARGET};

/// The generation the fixture's authority starts at, and the one it advances
/// to while a resolver keeps the first — both are declared data, and the
/// blocker names both numbers rather than the word "stale".
pub(crate) const HELD_GENERATION: u64 = 7;
pub(crate) const ADVANCED_GENERATION: u64 = 9;
/// One generation below what a live resolver holds: the value that makes the
/// next refresh a rollback the resolver must refuse.
pub(crate) const ROLLED_BACK_GENERATION: u64 = 6;

pub(crate) fn status(host: &Host) -> std::process::Output {
    host.stado(&["resolver", "status", "--target", TARGET, "--json"])
}

/// Bring a real resolver up and prove it is the one serving: both declared
/// binds accepted, and the published state names this process.
pub(crate) fn serving(host: &Host, policy: &Policy) -> (Serving, Value) {
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

