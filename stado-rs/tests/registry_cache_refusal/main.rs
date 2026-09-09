//! What the product does when the authority serves a registry document this
//! build refuses.
//!
//! # What happened
//!
//! The reader-side cache refused documents silently until 2026-09-03: each
//! refusal printed one line and the function returned `()`, so a host could
//! sit a registry generation behind indefinitely with nobody able to say so.
//! The rule the cache enforces is that a document this build does not accept
//! never replaces the copy already on disk — the copy is the only thing that
//! answers when the store is down, and a copy nobody can trust is a copy
//! nobody may use.
//!
//! # What is defended here, and through what
//!
//! Every case drives the built binary against an isolated storage root, a
//! `HOME` inside it and this machine declared as the target host, so the
//! current-host path really runs. The states asserted are the ones an operator
//! can reach: the refusal sentence the process prints, the exit status, the
//! `build-refuses-registry` finding `stado registry doctor` publishes, the
//! sentence `stado registry validate` prints for the same document, and — the
//! one that matters most — the bytes of the recorded copy and its sidecar,
//! read off disk after each refusal.
//!
//! Two conditions of the cache have no operator surface at all and are named
//! in the pull request rather than asserted here through a library call: a
//! document that is not JSON (the loader refuses it before the cache is ever
//! offered it) and the process-local record of a refusal (published only in
//! `stado resolver serve`'s readiness document, which no single invocation of
//! a one-shot command can both write and read).
//!
//! [`copy`] holds the cases about which document survives on disk, and
//! [`location`] the two about having somewhere to write it at all.

mod copy;
mod fixture;
mod location;

use crate::fixture::{
    accepted_document, contract_refusal, declares_an_unimplemented_key, report, stderr, stdout,
    Fixture, REFUSAL_PREFIX, TARGET,
};

/// The copy is written from the document the authority served, byte for byte,
/// and dated by a sidecar naming the generation. Everything below is a
/// mutation of this state, so it has to be established first.
#[test]
fn the_document_the_authority_serves_becomes_this_hosts_recorded_copy() {
    let fixture = Fixture::new();

    let output = fixture.read_registry();

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains(TARGET),
        "the command answers with this machine's target: {}",
        stdout(&output)
    );
    assert_eq!(
        fixture.recorded_copy().as_deref(),
        Some(accepted_document().as_str()),
        "the copy is the authority's own bytes"
    );
    assert!(
        !fixture.recorded_generation().is_empty(),
        "the sidecar names the generation the copy was read at"
    );
}

/// A document declaring a key this build has no implementation for is refused,
/// the older copy is left exactly as it was, and the command still answers —
/// the authority did answer, so the read is not degraded.
#[test]
fn a_document_declaring_a_key_this_build_does_not_implement_is_refused() {
    let fixture = Fixture::new();
    fixture.read_registry();
    let recorded = fixture.recorded_copy().expect("a copy was recorded");
    let generation = fixture.recorded_generation();

    fixture.publish(&declares_an_unimplemented_key());
    let output = fixture.read_registry();

    let complaint = stderr(&output);
    assert!(
        complaint.contains(REFUSAL_PREFIX)
            && complaint.contains(&fixture.cache_document().display().to_string())
            && complaint.contains(&contract_refusal()),
        "the refusal names the copy it did not write and the contract's own words: {complaint}"
    );
    assert!(
        output.status.success(),
        "a refused refresh does not fail the read the authority answered: {complaint}"
    );
    assert_eq!(
        fixture.recorded_copy().as_deref(),
        Some(recorded.as_str()),
        "the older copy is left byte-identical"
    );
    assert_eq!(
        fixture.recorded_generation(),
        generation,
        "the sidecar still dates the copy that is actually on disk"
    );
}

/// The same refusal as a finding an operator asks for by name. `registry
/// doctor` reports it against the host it is running on, exits non-zero, and
/// does not record the document either.
#[test]
fn registry_doctor_names_the_build_that_refuses_the_published_document() {
    let fixture = Fixture::new();
    fixture.read_registry();
    let recorded = fixture.recorded_copy().expect("a copy was recorded");

    fixture.publish(&declares_an_unimplemented_key());
    let output = fixture.stado(&["registry", "doctor", "--json"]);

    let document = report(&output);
    let finding = document["findings"]
        .as_array()
        .expect("the report lists what it found")
        .iter()
        .find(|finding| finding["finding"] == serde_json::json!("build-refuses-registry"))
        .unwrap_or_else(|| panic!("no build-refuses-registry finding in {document:#}"));
    assert_eq!(finding["subject"], serde_json::json!(TARGET));
    let detail = finding["detail"]
        .as_str()
        .expect("the finding is a sentence");
    assert!(
        detail.contains("rejected-by-this-build") && detail.contains(&contract_refusal()),
        "the finding names the refusal and the validator's own words: {detail}"
    );
    assert_eq!(
        document["ok"],
        serde_json::json!(false),
        "a refused document is not a healthy registry: {document:#}"
    );
    assert!(
        !output.status.success(),
        "a divergence is a failed verdict: {}",
        stderr(&output)
    );
    assert_eq!(
        fixture.recorded_copy().as_deref(),
        Some(recorded.as_str()),
        "reporting the refusal does not record the document either"
    );
}

/// The same gate, reached by hand before publishing. An operator holding the
/// document gets the identical sentence and a failed exit, and the document
/// this fixture calls accepted really is accepted — otherwise every refusal
/// here could be the fixture's fault.
#[test]
fn registry_validate_refuses_the_same_document_and_accepts_the_other() {
    let fixture = Fixture::new();
    let refused = fixture.path().join("refused.json");
    let accepted = fixture.path().join("accepted.json");
    std::fs::write(&refused, declares_an_unimplemented_key()).expect("write the refused document");
    std::fs::write(&accepted, accepted_document()).expect("write the accepted document");

    let refusal = fixture.stado(&["registry", "validate", &refused.display().to_string()]);
    assert!(!refusal.status.success());
    assert!(
        stderr(&refusal).contains(&contract_refusal()),
        "the validator prints its own words: {}",
        stderr(&refusal)
    );

    let acceptance = fixture.stado(&["registry", "validate", &accepted.display().to_string()]);
    assert!(acceptance.status.success(), "{}", stderr(&acceptance));
    assert!(
        stdout(&acceptance).contains("valid registry"),
        "{}",
        stdout(&acceptance)
    );
}
