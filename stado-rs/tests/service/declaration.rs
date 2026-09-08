//! The declaration contract: what the service directory accepts, and what it
//! refuses without moving.
//!
//! These cases were already driven by the real binary; what was fake was the
//! fleet they ran against — a target named `w1` with the destination
//! `u@10.0.0.1`, a machine that does not exist. The host named here is this
//! one, so the accepted declaration's endpoint, active host and per-host
//! record are all about a machine the test is standing on.
//!
//! The authority target declares `<account>@127.0.0.1` because the directory
//! contract refuses a directory whose authority has no connection path
//! (`registry.service_directory.authority.target: must declare an SSH
//! connection path`). `declare` reads and writes the document and touches no
//! host, so that destination is never dialled; the lifecycle cases in the
//! rest of this area declare no destination at all.

use serde_json::{json, Value};

use crate::fixture::fleet::{json_stdout, said, Fleet, FIRST_GENERATION, PORT, SHA256_HEX_LEN};

const SERVICE: &str = "stado-service-area-serving";
const CONSUMER: &str = "stado-service-area-tests";
const ARTIFACT: &str = "stado://releases/stado-service-area-serving/1.0.0/darwin-arm64";

/// A declaration the contract accepts, as a document this test can bend one
/// field of at a time.
fn declaration(fleet: &Fleet) -> Value {
    json!({
        "name": SERVICE,
        "host": fleet.target,
        "port": PORT,
        "source": {"artifact": ARTIFACT, "sha256": "0".repeat(SHA256_HEX_LEN)},
        "run": {"program": PROGRAM, "args": ["serve"]},
        "consumers": {CONSUMER: {"capabilities": ["model-routing"]}},
    })
}

/// The program a declaration names on this machine. A real executable,
/// because a declaration naming nothing is a different case.
const PROGRAM: &str = "/bin/sleep";

fn declare(fleet: &Fleet, document: &Value) -> std::process::Output {
    let path = fleet.root().join("declaration.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(document).expect("a declaration document"),
    )
    .expect("write the declaration");
    fleet.stado(&[
        "service",
        "declare",
        "--file",
        path.to_str().expect("a UTF-8 declaration path"),
        "--json",
    ])
}

/// Refuse `document`, assert the sentence, and assert the persisted registry
/// did not move one byte.
fn refused(fleet: &Fleet, document: &Value, sentence: &str) {
    let before = fleet.registry_bytes();
    let out = declare(fleet, document);
    assert!(!out.status.success(), "declare accepted {document}");
    assert!(
        said(&out).contains(sentence),
        "expected {sentence:?}, got: {}",
        said(&out)
    );
    assert_eq!(
        fleet.registry_bytes(),
        before,
        "a refused declaration moved the document"
    );
}

#[test]
fn a_declaration_lands_in_the_persisted_directory_with_its_endpoint_and_consumers() {
    let fleet = Fleet::directory();
    let out = declare(&fleet, &declaration(&fleet));
    assert!(out.status.success(), "declare failed: {}", said(&out));
    let report = json_stdout(&out);
    assert_eq!(report["declared"], SERVICE);
    assert_eq!(report["host"], fleet.target);

    let document = fleet.registry();
    let directory = &document["service_directory"];
    // A consumer holding the previous generation must be able to tell that
    // this entry is new, so the counter has to advance with the write.
    assert!(
        directory["generation"].as_u64().expect("a generation") > FIRST_GENERATION,
        "the publication counter did not advance: {directory}"
    );
    let entry = &directory["services"][SERVICE];
    assert_eq!(entry["active_host"], fleet.target);
    assert_eq!(
        entry["endpoints"][&fleet.target]["url"],
        format!("http://127.0.0.1:{PORT}")
    );
    assert_eq!(entry["managed_service"], SERVICE);
    assert_eq!(
        entry["consumers"][CONSUMER]["capabilities"][0],
        "model-routing"
    );
    assert_eq!(entry["declaration"]["source"]["artifact"], ARTIFACT);
    assert_eq!(
        entry["declaration"]["source"]["sha256"],
        "0".repeat(SHA256_HEX_LEN)
    );
    assert_eq!(entry["declaration"]["run"]["args"][0], "serve");

    // The declared-but-not-yet-deployed record on this machine's own target.
    let record = crate::only_record(&fleet);
    assert_eq!(record["name"], SERVICE);
    assert_eq!(record["declared_only"], true);
}

#[test]
fn a_digest_that_is_not_immutable_is_refused() {
    let fleet = Fleet::directory();
    let mut document = declaration(&fleet);
    document["source"]["sha256"] = json!("ABC123");
    refused(
        &fleet,
        &document,
        "declaration.source.sha256: must be 64 lowercase hex characters",
    );
}

#[test]
fn a_host_outside_the_registry_is_refused() {
    let fleet = Fleet::directory();
    let mut document = declaration(&fleet);
    document["host"] = json!("no-such-machine");
    refused(
        &fleet,
        &document,
        "'host' names no-such-machine, which is not a registry target",
    );
}

#[test]
fn a_declaration_naming_no_consumer_is_refused() {
    let fleet = Fleet::directory();
    let mut document = declaration(&fleet);
    document
        .as_object_mut()
        .expect("a declaration object")
        .remove("consumers");
    refused(
        &fleet,
        &document,
        "'consumers' is required and must name at least one caller",
    );
}

#[test]
fn a_verify_kind_this_build_does_not_implement_is_refused_with_the_ones_it_does() {
    let fleet = Fleet::directory();
    let mut document = declaration(&fleet);
    document["verify"] = json!({"kind": "dns"});
    refused(
        &fleet,
        &document,
        "verify.kind: unknown value 'dns'; this build implements ['http', 'tcp']",
    );
}

#[test]
fn a_name_with_empty_edges_is_refused() {
    let fleet = Fleet::directory();
    let mut document = declaration(&fleet);
    document["name"] = json!("-stado-area-");
    refused(
        &fleet,
        &document,
        "'name' must be a lowercase identifier without empty edges",
    );
}

#[test]
fn a_declaration_with_no_endpoint_for_its_own_host_is_refused() {
    let fleet = Fleet::directory();
    let mut document = declaration(&fleet);
    document
        .as_object_mut()
        .expect("a declaration object")
        .remove("port");
    refused(
        &fleet,
        &document,
        &format!(
            "the declaration needs an endpoint for {} — pass 'endpoints' or the 'port' shorthand",
            fleet.target
        ),
    );
}
