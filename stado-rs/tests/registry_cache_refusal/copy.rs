//! Which document survives on this host's disk, and what answers from it.

use crate::fixture::{
    declares_an_unimplemented_key, names_no_hosts, stderr, stdout, Fixture, REFUSAL_PREFIX, TARGET,
};

/// Valid is not the same as better. A document naming no hosts passes the
/// contract and still never replaces a copy that names some — on 2026-09-01 it
/// did, and the fleet's own recovery copy was destroyed by its own refresh.
#[test]
fn a_document_naming_no_hosts_never_replaces_a_copy_that_names_some() {
    let fixture = Fixture::new();
    fixture.read_registry();
    let recorded = fixture.recorded_copy().expect("a copy was recorded");

    fixture.publish(&names_no_hosts());
    let output = fixture.read_registry();

    let complaint = stderr(&output);
    assert!(
        complaint.contains(REFUSAL_PREFIX)
            && complaint.contains(
                "the authority served a registry naming no hosts and the recorded copy names 1; \
                 keeping the recorded copy, because an empty fleet is what an outage looks like \
                 from here"
            ),
        "the refusal keeps the safeguard's own words: {complaint}"
    );
    // What an empty fleet does to the operator asking a question, and why the
    // copy is worth keeping: the authority answered, so the answer came from
    // the document naming nobody, and the refusal names the object it read.
    assert!(
        !output.status.success() && complaint.contains("is not in stado://probierz/registry.json"),
        "the served document names no host, so this machine cannot be resolved: {complaint}"
    );
    assert_eq!(
        fixture.recorded_copy().as_deref(),
        Some(recorded.as_str()),
        "the safeguard keeps the copy that names hosts"
    );
}

/// The rule is about losing hosts, not about being empty: a fresh install
/// legitimately declares none and must still be able to record a copy.
#[test]
fn a_document_naming_no_hosts_is_recorded_when_this_host_holds_none() {
    let fixture = Fixture::empty();
    fixture.publish(&names_no_hosts());

    let output = fixture.read_registry();

    assert!(
        !stderr(&output).contains(REFUSAL_PREFIX),
        "nothing was refused: {}",
        stderr(&output)
    );
    assert_eq!(
        fixture.recorded_copy().as_deref(),
        Some(names_no_hosts().as_str()),
        "a fresh install is cacheable"
    );
}

/// The consequence of every refusal, and the reason the rule exists: the copy
/// those refusals preserved is what answers once the authority stops
/// answering, and it still names this machine.
#[test]
fn the_preserved_copy_is_what_answers_when_the_authority_goes_away() {
    let fixture = Fixture::new();
    fixture.read_registry();
    let generation = fixture.recorded_generation();
    fixture.publish(&declares_an_unimplemented_key());
    fixture.read_registry();
    fixture.publish(&names_no_hosts());
    fixture.read_registry();

    fixture.withdraw();
    let output = fixture.read_registry();

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains(TARGET),
        "the copy still names this machine: {}",
        stdout(&output)
    );
    assert_eq!(
        fixture.recorded_generation(),
        generation,
        "the generation served is the one the two refusals preserved"
    );
    let notice = stderr(&output);
    assert!(
        notice.contains("reading the last-known-good registry copy from")
            && notice.contains(&fixture.cache_document().display().to_string())
            && notice.contains(&format!("generation {generation}"))
            && notice.contains(
                "because the authority did not answer: registry store unreachable \
                 (stado://probierz/registry.json)"
            )
            && notice.contains("HTTP 503"),
        "the degraded read says what it is reading, how old it is and why: {notice}"
    );
}
