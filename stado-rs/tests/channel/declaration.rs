//! What the canonical registry accepts and refuses for a public release
//! origin declared against the machine running these cases, and what it
//! leaves on disk either way.
//!
//! The subject of every case is this host: the target names the kernel's own
//! answer, so the product takes its current-host path, and the origin under
//! judgement is this machine's own name. That name is exactly the interesting
//! case — a workstation answers to it on its own network and no public
//! resolver has ever heard of it, which is the confusion `public_origins`
//! exists to refuse.

use serde_json::{json, Value};

use crate::fixture::{self, only_row, report, stderr, Fixture, ORIGIN, PUBLISHED_PATH, TARGET};
use crate::listeners::Upstream;

/// A target name the seeded registry does not declare. It is the subject of
/// the case below — the document is about a target that does not exist — and
/// never a stand-in for a host that does.
const UNDECLARED_TARGET: &str = "no-such-target-in-this-registry";

fn declared_document(hostname: &str, target: &str, upstream: &str) -> Value {
    let mut document = fixture::current_host_registry();
    document["public_origins"] = fixture::origin_row(hostname, target, upstream);
    document
}

/// A live upstream is not a public origin. The declaration names a loopback
/// socket this test really bound and really proves is accepting, and the write
/// is still refused, because publicness is a fact about the NAME.
#[test]
fn declaring_this_hosts_own_name_over_a_live_upstream_is_refused_and_writes_nothing() {
    let fixture = Fixture::new();
    let upstream = Upstream::bind();
    assert!(
        crate::listeners::accepts(upstream.port()),
        "the case's premise is a live upstream, and 127.0.0.1:{} did not accept",
        upstream.port()
    );
    let hostname = fixture::hostname();
    let refused = fixture.stado(&[
        "web",
        "origin",
        "declare",
        ORIGIN,
        "--hostname",
        &hostname,
        "--target",
        TARGET,
        "--upstream",
        &upstream.origin(),
        "--path",
        PUBLISHED_PATH,
        "--json",
    ]);

    assert_eq!(refused.status.code(), Some(1));
    let complaint = stderr(&refused);
    assert!(
        complaint.contains(&format!(
            "refusing to declare public origin {ORIGIN:?}: {hostname} has no public A or AAAA \
             record, so no public edge could fetch it; publish the name first, then declare it"
        )),
        "the refusal must name the origin, the hostname and the repair: {complaint}"
    );
    assert_eq!(
        fixture.persisted_origins(),
        None,
        "a refused declaration must leave no public_origins key on disk"
    );
    let listed = fixture.stado(&["web", "origin", "list", "--json"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert_eq!(
        report(&listed),
        json!([]),
        "a refused declaration lists no row"
    );
}

/// `/docs/channels`: a host-control route and a public download origin are
/// separate choices. This host's own name is both here — it is the SSH
/// destination the target declares — so the write is refused and the store is
/// byte-for-byte what it was.
#[test]
fn an_origin_derived_from_this_hosts_control_route_is_refused_and_the_store_is_unchanged() {
    let fixture = Fixture::new();
    let control_route = fixture::control_route_registry();
    fixture.seed(&control_route);
    let before = fixture.registry_bytes();
    let hostname = fixture::hostname();
    let upstream = Upstream::bind();
    let mut document = control_route;
    document["public_origins"] = fixture::origin_row(&hostname, TARGET, &upstream.origin());

    let refusal = format!(
        "registry.public_origins[0].hostname {hostname} is a declared host-control destination; \
         a public origin is a separate choice from the route Stado reaches the host on and must \
         not be derived from it"
    );
    let validated = fixture.validate(&document);
    assert_eq!(validated.status.code(), Some(1));
    assert!(
        stderr(&validated).contains(&refusal),
        "validation must name the derivation it refuses: {}",
        stderr(&validated)
    );

    let pushed = fixture.push(&document);
    assert_eq!(pushed.status.code(), Some(1));
    assert!(
        stderr(&pushed).contains(&refusal),
        "the write must refuse it in the same words: {}",
        stderr(&pushed)
    );
    assert_eq!(
        fixture.registry_bytes(),
        before,
        "a refused write must not touch the canonical document"
    );
}

/// An origin whose target nothing declares cannot be converged by anything, so
/// the document is refused before it is stored.
#[test]
fn an_origin_naming_an_undeclared_target_is_refused_and_the_store_is_unchanged() {
    let fixture = Fixture::new();
    let before = fixture.registry_bytes();
    let upstream = Upstream::bind();
    let document = declared_document(&fixture::hostname(), UNDECLARED_TARGET, &upstream.origin());

    let pushed = fixture.push(&document);
    assert_eq!(pushed.status.code(), Some(1));
    let complaint = stderr(&pushed);
    assert!(
        complaint.contains(&format!(
            "registry.public_origins[0].target names no declared target: {UNDECLARED_TARGET:?}"
        )),
        "the refusal must name the target nothing declares: {complaint}"
    );
    assert_eq!(fixture.registry_bytes(), before);
    assert_eq!(fixture.persisted_origins(), None);
}

/// The accepted write, read back off the disk it landed on and then withdrawn.
///
/// The declaration is made through `registry push` rather than `web origin
/// declare`, because this machine's own name has no public address record and
/// the declare command refuses it for that reason — proved by the first case
/// in this file. Both go through the same validated commit, and what is under
/// judgement here is the persisted document.
#[test]
fn an_accepted_declaration_is_persisted_on_disk_and_then_withdrawn_from_it() {
    let fixture = Fixture::new();
    let upstream = Upstream::bind();
    let hostname = fixture::hostname();
    let declared = fixture::origin_row(&hostname, TARGET, &upstream.origin());
    let document = declared_document(&hostname, TARGET, &upstream.origin());

    let pushed = fixture.push(&document);
    assert!(pushed.status.success(), "{}", stderr(&pushed));
    assert_eq!(
        fixture.persisted_origins(),
        Some(declared),
        "the accepted declaration must be the row the canonical document carries"
    );

    let listed = fixture.stado(&["web", "origin", "list", "--json"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let row = only_row(&report(&listed)).clone();
    assert_eq!(row["name"], json!(ORIGIN));
    assert_eq!(row["hostname"], json!(hostname));
    assert_eq!(
        row["origin"],
        json!(format!("https://{hostname}")),
        "a public origin is HTTPS or it is not public"
    );
    assert_eq!(row["upstream"], json!(upstream.origin()));
    assert_eq!(row["paths"], json!([PUBLISHED_PATH]));

    let removed = fixture.stado(&["web", "origin", "remove", ORIGIN, "--json"]);
    assert!(removed.status.success(), "{}", stderr(&removed));
    let receipt = report(&removed);
    assert_eq!(receipt["change"], json!("removed"));
    assert_eq!(receipt["hostname"], json!(hostname));
    assert_eq!(
        fixture.persisted_origins(),
        None,
        "a withdrawn declaration must leave no empty key behind on disk"
    );
    let targets = fixture.registry()["targets"].clone();
    assert_eq!(
        targets[0]["hostnames"],
        json!([hostname]),
        "withdrawing an origin must not disturb the target that published it"
    );
}
