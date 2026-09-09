//! A host whose installed build refuses the registry the control plane
//! publishes, read off the surface an operator reaches for.
//!
//! On `lukasz-macbook` the disk janitor recorded `policy:ValueError` 8,348
//! times across roughly 12,700 passes in two windows, 2026-08-20 to 08-27 and
//! 08-31 to 09-02. The registry was valid throughout; the running build was
//! too old to accept it. Both windows opened with no restart and no binary
//! replacement, and both closed on an unrelated restart onto a newer build —
//! which is why `stale-unit-image` fires nothing here: the installed file and
//! the running image agreed, and the registry was what moved. The janitor
//! learned to journal the refusal, and `resolver status` learned to publish it
//! as a blocker, and this area defends the two verdicts a reader gets from
//! `stado registry doctor`: a build that refuses the document, and a build
//! nobody could ask.
//!
//! Every case drives the built binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local, WC_LOCAL_STORAGE_PATH inside a tempdir, HOME
//! inside that tempdir, and STADO_CONFIG pointing at a path that does not
//! exist, so the operator's own registry, cache and configuration can never
//! reach a run. The machine is declared by its own kernel host name, so the
//! product's current-host resolution is what decides which target these
//! verdicts are about. Every asserted sentence was copied from a hand run
//! against this seeded state.
//!
//! Rewritten on 2026-09-08: the previous version called
//! `targets::builds_refusing_registry` in-process and never ran the product,
//! so nothing it asserted was evidence about a command anybody types.

mod fixture;

use fixture::{
    accepted_cleaners, cleaner_a_newer_build_knows, cleaner_with_an_unimplemented_key, detail,
    findings, stderr, stdout, Harness, LOCAL, REFUSES, REMOTE, UNIMPLEMENTED_KEY, UNREAD,
};

/// The refusal is about the machine that refused, it names the installed
/// version, it carries the validator's own words, and it says what clears it.
#[test]
fn a_build_that_refuses_the_document_reports_it_about_itself() {
    let harness = Harness::new();
    harness.declare_registry(cleaner_with_an_unimplemented_key());

    let out = harness.stado(&["registry", "doctor", "--json"]);
    let rows = findings(&out, REFUSES);

    assert_eq!(rows.len(), 1, "one row for the one host that was asked");
    assert_eq!(
        rows[0].get("subject").and_then(serde_json::Value::as_str),
        Some(LOCAL),
        "the row names the host that answered, not an unread machine"
    );

    let sentence = detail(&rows[0]);
    assert!(
        sentence.contains(&format!("({})", harness.installed_version())),
        "the sentence names the installed build: {sentence}"
    );
    assert!(
        sentence.contains(UNIMPLEMENTED_KEY),
        "and therefore names what was refused: {sentence}"
    );
    assert!(
        sentence.contains("republishing the registry does not"),
        "and says what does not clear it: {sentence}"
    );
    assert!(
        !out.status.success(),
        "a divergence exits non-zero: {sentence}"
    );
}

/// The other surface refuses the same document with the same clause, and
/// writes nothing while doing it.
#[test]
fn registry_validate_refuses_the_same_document_and_leaves_it_alone() {
    let harness = Harness::new();
    harness.declare_registry(cleaner_with_an_unimplemented_key());
    let before = std::fs::read(harness.registry_path()).expect("read the seeded document");

    let out = harness.validate();

    assert!(!out.status.success(), "an unreadable document is refused");
    let refusal = stderr(&out);
    assert!(
        refusal.contains(&format!(
            "registry.targets[0].disk_cleanup.cleaners.build_caches: unknown keys ['{UNIMPLEMENTED_KEY}']"
        )),
        "the refusal names the key and where it sits: {refusal}"
    );
    assert_eq!(
        std::fs::read(harness.registry_path()).expect("read the document back"),
        before,
        "a refusal writes nothing"
    );
}

/// A document this build accepts produces no verdict about this machine at
/// all, on either surface.
#[test]
fn a_document_this_build_accepts_produces_no_refusal() {
    let harness = Harness::new();
    harness.declare_registry(accepted_cleaners());

    let validated = harness.validate();
    assert!(
        validated.status.success(),
        "the accepted document validates: {}",
        stderr(&validated)
    );
    assert!(
        stdout(&validated).contains(&format!(
            "valid registry: {}",
            harness.registry_path().display()
        )),
        "and says which document it read: {}",
        stdout(&validated)
    );

    let doctor = harness.stado(&["registry", "doctor", "--json"]);
    let about_this_machine: Vec<serde_json::Value> = findings(&doctor, REFUSES)
        .into_iter()
        .filter(|row| row.get("subject").and_then(serde_json::Value::as_str) == Some(LOCAL))
        .collect();
    assert!(
        about_this_machine.is_empty(),
        "no refusal about the host that accepted it: {about_this_machine:?}"
    );
}

/// A machine this process cannot ask is unmeasured, and unmeasured is not
/// acceptance and not refusal. The sentence has to say who was running and
/// where, or a reader cannot tell which of the two silences they are holding.
#[test]
fn a_machine_this_process_cannot_ask_is_unmeasured_not_refused() {
    let harness = Harness::new();
    harness.declare_registry(accepted_cleaners());

    let out = harness.stado(&["registry", "doctor", "--json"]);

    let unread = findings(&out, UNREAD);
    assert_eq!(unread.len(), 1, "one row for the one machine nobody asked");
    assert_eq!(
        unread[0].get("subject").and_then(serde_json::Value::as_str),
        Some(REMOTE)
    );
    let sentence = detail(&unread[0]);
    assert!(
        sentence.contains("is NOT reported as acceptance"),
        "silence is not acceptance: {sentence}"
    );
    assert!(
        sentence.contains(&harness.installed_version()) && sentence.contains(LOCAL),
        "the sentence names the build that ran and where it ran: {sentence}"
    );

    let refused_remote: Vec<serde_json::Value> = findings(&out, REFUSES)
        .into_iter()
        .filter(|row| row.get("subject").and_then(serde_json::Value::as_str) == Some(REMOTE))
        .collect();
    assert!(
        refused_remote.is_empty(),
        "an unasked machine is never reported as refusing: {refused_remote:?}"
    );
}

/// Both kinds arrive in one run, on two different subjects: the machine that
/// answered refuses, the machine nobody could ask is unmeasured.
#[test]
fn the_two_kinds_are_distinct_in_one_report() {
    let harness = Harness::new();
    harness.declare_registry(cleaner_with_an_unimplemented_key());

    let out = harness.stado(&["registry", "doctor", "--json"]);

    let refused = findings(&out, REFUSES);
    let unread = findings(&out, UNREAD);
    assert_eq!(refused.len(), 1);
    assert_eq!(unread.len(), 1);
    assert_eq!(
        refused[0]
            .get("subject")
            .and_then(serde_json::Value::as_str),
        Some(LOCAL)
    );
    assert_eq!(
        unread[0].get("subject").and_then(serde_json::Value::as_str),
        Some(REMOTE)
    );
}

/// A cleaner *name* this build does not know is skipped, not refused. This is
/// the case the area used to assert backwards: refusing an unfamiliar name
/// switched every cleaner off on a host whose document had simply moved ahead
/// of its binary, so the product changed and the check has to follow.
#[test]
fn a_cleaner_name_this_build_does_not_know_is_not_a_refusal() {
    let harness = Harness::new();
    harness.declare_registry(cleaner_a_newer_build_knows());

    let validated = harness.validate();
    assert!(
        validated.status.success(),
        "an unfamiliar cleaner name is a document this build can still read: {}",
        stderr(&validated)
    );

    let doctor = harness.stado(&["registry", "doctor", "--json"]);
    let about_this_machine: Vec<serde_json::Value> = findings(&doctor, REFUSES)
        .into_iter()
        .filter(|row| row.get("subject").and_then(serde_json::Value::as_str) == Some(LOCAL))
        .collect();
    assert!(
        about_this_machine.is_empty(),
        "and therefore no refusal about this machine: {about_this_machine:?}"
    );
}
