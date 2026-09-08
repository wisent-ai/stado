//! Real evidence for route declaration and resolution.
//!
//! Every case here drives the built `stado` binary against an isolated local
//! registry whose one target names the machine running the test, so the
//! production host channel takes its current-host path and executes the
//! operating system's own tools. Nothing is mocked, no executable is
//! substituted, and no assertion rests on stdout alone: each one reads the
//! state the command left behind — the canonical registry document, the
//! forward marker on disk, the broker's capability route table, the exit code.
//!
//! The four things the capability promises, and where each is proved:
//!
//! * declaring a route and reading it back out of the persisted registry —
//!   [`declaring_a_route_writes_it_into_the_persisted_service_directory`];
//! * resolving a declared route through the real broker — `capability.rs`;
//! * the refusal for an unknown route —
//!   [`an_unknown_route_is_refused_with_the_declaration_to_edit`];
//! * the refusal for a declaration that names no resource —
//!   [`a_declaration_naming_no_endpoint_is_refused_and_writes_nothing`].

#[path = "../support/skarbiec.rs"]
mod skarbiec_support;

mod broker;
mod capability;
mod fleet;

use std::os::unix::fs::PermissionsExt;

use serde_json::Value;

use fleet::{json_stdout, said, Fleet, PORT, SERVICE, TARGET};

/// The canonical registry document's own schema field, and the generation the
/// isolated directory starts at. Both are contract, not tuning: `declare` must
/// advance the second one or every cached copy stays blind to the new entry.
pub const REGISTRY_SCHEMA_VERSION: u64 = 2;
pub const FIRST_GENERATION: u64 = 1;
/// A declaration's digest is 64 lowercase hex characters; the validator says so.
pub const SHA256_HEX_LEN: usize = 64;

fn declared_url() -> String {
    format!("http://127.0.0.1:{PORT}")
}

fn declare(fleet: &Fleet) {
    let file = fleet.declaration(SERVICE, true);
    let output = fleet.stado(&[
        "service",
        "declare",
        "--file",
        file.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        output.status.success(),
        "declaring the route failed:\n{}",
        said(&output)
    );
}

#[test]
fn declaring_a_route_writes_it_into_the_persisted_service_directory() {
    let fleet = Fleet::new();
    declare(&fleet);

    // The persisted document, not the report the command printed.
    let document = fleet.registry();
    let entry = &document["service_directory"]["services"][SERVICE];
    assert_eq!(entry["active_host"], TARGET);
    assert_eq!(entry["endpoints"][TARGET]["url"], declared_url());
    assert_eq!(entry["managed_service"], SERVICE);
    assert_eq!(
        entry["declaration"]["run"]["program"], "/usr/bin/true",
        "the declaration did not travel with the directory entry"
    );
    assert!(
        document["service_directory"]["generation"]
            .as_u64()
            .is_some_and(|generation| generation > FIRST_GENERATION),
        "the publication counter did not advance, so no consumer learns the route exists"
    );
    assert_eq!(
        document["targets"][0]["services"][0]["name"], SERVICE,
        "the host that must run it was not linked to the declaration"
    );

    // And the product reads its own write back through the route surface.
    let listed = fleet.stado(&["route", "list", "--json"]);
    assert!(listed.status.success(), "{}", said(&listed));
    let report = json_stdout(&listed);
    let row = report["services"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["service"] == SERVICE))
        .unwrap_or_else(|| panic!("{SERVICE} is absent from {}", said(&listed)));
    assert_eq!(row["active_host"], TARGET);
    assert_eq!(row["endpoints"][0]["target"], TARGET);
    assert_eq!(row["endpoints"][0]["url"], declared_url());
    assert_eq!(row["local_forward"], Value::Null);
}

#[test]
fn opening_the_declared_route_writes_the_endpoint_and_closing_removes_it() {
    let fleet = Fleet::new();
    declare(&fleet);
    let declared = fleet.registry();

    let opened = fleet.stado(&["route", "open", SERVICE, "--local", "--json"]);
    assert!(opened.status.success(), "{}", said(&opened));
    let marker = fleet.marker(SERVICE);
    assert_eq!(
        json_stdout(&opened)["forward"]["marker"],
        marker.to_str().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        format!("{}\n", declared_url()),
        "the marker a credential bridge reads does not carry the declared endpoint"
    );
    assert_eq!(
        std::fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(marker.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let listed = json_stdout(&fleet.stado(&["route", "list", "--json"]));
    assert_eq!(
        listed["services"][0]["local_forward"]["url"],
        declared_url()
    );

    let closed = fleet.stado(&["route", "close", SERVICE]);
    assert!(closed.status.success(), "{}", said(&closed));
    assert!(
        !marker.exists(),
        "close left the credential bridge reading a stale endpoint"
    );
    assert_eq!(
        fleet.registry(),
        declared,
        "opening and closing a forward rewrote the declaration"
    );
}

#[test]
fn an_unknown_route_is_refused_with_the_declaration_to_edit() {
    let fleet = Fleet::new();
    declare(&fleet);
    let declared = fleet.registry();

    let output = fleet.stado(&["route", "open", "no-such-route", "--local"]);
    assert_eq!(output.status.code(), Some(1), "{}", said(&output));
    assert!(
        said(&output).contains(
            "no-such-route is not in the service directory; add it to service_directory.services"
        ),
        "{}",
        said(&output)
    );
    assert!(!fleet.marker("no-such-route").exists());
    assert_eq!(fleet.registry(), declared);

    // A route that exists but no direction to materialize it in is a usage
    // error, and must not write a marker either.
    let undirected = fleet.stado(&["route", "open", SERVICE]);
    assert_eq!(undirected.status.code(), Some(2), "{}", said(&undirected));
    assert!(
        said(&undirected).contains(
            "route open requires --local or --remote; choose where the declared forward marker must live"
        ),
        "{}",
        said(&undirected)
    );
    assert!(!fleet.marker(SERVICE).exists());
    assert_eq!(fleet.registry(), declared);
}

#[test]
fn a_declaration_naming_no_endpoint_is_refused_and_writes_nothing() {
    let fleet = Fleet::new();
    let before = std::fs::read(fleet.registry_path()).unwrap();
    let file = fleet.declaration("route-real-endpointless", false);

    let output = fleet.stado(&[
        "service",
        "declare",
        "--file",
        file.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2), "{}", said(&output));
    assert!(
        said(&output).contains(&format!(
            "{}: the declaration needs an endpoint for {TARGET} — pass 'endpoints' or the 'port' shorthand",
            file.display()
        )),
        "{}",
        said(&output)
    );
    assert_eq!(
        std::fs::read(fleet.registry_path()).unwrap(),
        before,
        "a refused declaration still moved the canonical document"
    );
}
