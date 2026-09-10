//! Resolving a declared service on this machine, end to end.
//!
//! The declared endpoint travels three ways here and each one is checked
//! against something the product left behind: into the CLI's report, into the
//! forward marker on disk, and — with a real `resolver serve` in front of the
//! real object API — into the HTTP answer a consumer reads on its own
//! loopback port. The active host is this machine, so the adapter takes its
//! local-upstream path and no connection to any other host is opened.


use serde_json::json;

use crate::fixture::Policy;
use crate::{report, said, Host, CONSUMER, SERVICE, TARGET};

/// The generation this area's authority publishes.
pub(crate) const GENERATION: u64 = 7;
/// Reads issued at once. Large enough that one process per read would be
/// unmistakable in the child count, which is the shape that walked the
/// resolver into its own descriptor budget.
pub(crate) const CONCURRENT_READS: usize = 24;

#[test]
fn a_declared_service_resolves_to_its_endpoint_and_the_marker_lands_on_disk() {
    let policy = Policy::patient(GENERATION);
    let host = Host::new(&policy.document());
    let endpoint = format!("http://127.0.0.1:{}", policy.upstream);

    let answer = host.stado(&[
        "resolver",
        "resolve",
        SERVICE,
        "--consumer",
        CONSUMER,
        "--json",
    ]);
    assert!(
        answer.status.success(),
        "resolving a declared service failed: {}",
        said(&answer)
    );
    assert_eq!(
        report(&answer),
        json!({
            "service": format!("stado://service/{SERVICE}"),
            "generation": GENERATION,
            "capabilities": ["object-store"],
        })
    );

    let answer = host.stado(&["route", "list", "--json"]);
    assert!(answer.status.success(), "{}", said(&answer));
    let listed = report(&answer);
    assert_eq!(listed["authority"]["target"], TARGET);
    assert_eq!(listed["services"][0]["service"], SERVICE);
    assert_eq!(listed["services"][0]["active_host"], TARGET);
    assert_eq!(listed["services"][0]["endpoints"][0]["target"], TARGET);
    assert_eq!(listed["services"][0]["endpoints"][0]["url"], endpoint);
    assert_eq!(
        listed["services"][0]["local_forward"],
        serde_json::Value::Null,
        "nothing has been opened yet: {listed}"
    );

    let answer = host.stado(&["route", "open", SERVICE, "--local", "--json"]);
    assert!(answer.status.success(), "{}", said(&answer));
    let opened = report(&answer);
    assert_eq!(opened["status"], "open");
    assert_eq!(opened["endpoint"], endpoint);
    assert_eq!(opened["forward"]["location"], "local");
    assert_eq!(
        opened["forward"]["marker"],
        host.marker(SERVICE).display().to_string()
    );
    // The report is corroboration; this is the state the command left.
    assert_eq!(
        std::fs::read_to_string(host.marker(SERVICE)).expect("the marker was written"),
        format!("{endpoint}\n")
    );

    let answer = host.stado(&["route", "close", SERVICE]);
    assert!(answer.status.success(), "{}", said(&answer));
    assert!(
        !host.marker(SERVICE).exists(),
        "closing the forward left its marker behind"
    );
}

