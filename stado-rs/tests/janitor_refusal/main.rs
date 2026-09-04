//! The janitor's journal must tell a refused registry apart from a broken one.
//!
//! `resolve_canonical_policy` gates on `validate_registry`, which rejects a
//! registry that declares anything the running build has no implementation
//! for. Between 2026-08-20 and 2026-09-02 that gate fired 8348 times on
//! `lukasz-macbook` in two windows, and every one of them was journalled as
//! `policy:ValueError` — the same entry a corrupt document produces. The
//! document was never corrupt. It was valid, and the process holding it was
//! older than it: builds 0.7.14 through 0.7.22 refuse today's registry over an
//! unknown cleaner name, 0.13.24 refuses it over a changed required field set,
//! and 0.13.46 onward accept it. Neither window opened on a restart and both
//! closed on one, so the only signal an operator had said "invalid registry"
//! while the actual fault was the age of the running binary.
//!
//! What is defended here: a validation refusal is journalled
//! `policy:NotImplementedError`, a document that does not parse is STILL
//! journalled `policy:ValueError` exactly as before, both refusal shapes that
//! were observed in the field land on the refusal code, and the entry stays
//! bounded to `<stage>:<code>` with no field path, message text, or version.

use serde_json::{json, Value};
use stado::providers::local::disk_cleanup::{
    resolve_canonical_policy, CleanupReport, JanitorError,
};

/// The host this fixture speaks for. `resolve_canonical_policy` matches on
/// normalized identities, and a lowercase bare name normalizes to itself.
const HOST: &str = "t1";

/// A registry-v2 document with one local host whose `disk_cleanup` names the
/// full required field set and one cleaner this build implements. The current
/// build accepts it, which is what makes every mutation below attributable to
/// the mutation alone.
fn accepted_registry() -> Value {
    json!({
        "schema_version": 2,
        "coordinators": [],
        "targets": [
            {
                "name": HOST,
                "kind": "local",
                "ssh": "u@10.0.0.1",
                "release_platform": "darwin-arm64",
                "hostnames": ["t1.local"],
                "disk_cleanup": {
                    "mode": "report",
                    "check_interval_seconds": 3600,
                    "low_free_gb": 100,
                    "target_free_gb": 200,
                    "max_bytes_per_pass": 64 * 1024_i64.pow(3),
                    "max_items_per_pass": 512,
                    "max_scan_items": 4096,
                    "cleaners": {
                        "build_caches": { "min_age_seconds": 86400 }
                    }
                }
            }
        ]
    })
}

/// The journal entry the janitor would append for `exc`, produced by the same
/// call the pass makes: `report.add_error("policy", &exc)`.
fn journal_entry(exc: &JanitorError) -> String {
    let mut report = CleanupReport::base(0, HOST);
    report.add_error("policy", exc);
    assert_eq!(report.errors.len(), 1, "one failure, one entry");
    report.errors.remove(0)
}

/// The fixture must resolve cleanly, or a refusal below could be the fixture's
/// fault rather than the mutation's.
#[test]
fn the_fixture_registry_resolves() {
    let (target, policy, digest, defaulted) =
        resolve_canonical_policy(&accepted_registry(), HOST).expect("fixture registry resolves");
    assert_eq!(target.name, HOST);
    assert_eq!(policy.mode, "report");
    assert_eq!(digest.len(), 64, "policy digest is a sha256 hex string");
    assert!(!defaulted, "the fixture declares a policy");
}

/// Window one's shape: a cleaner name the build has no implementation for.
/// This is what 0.7.14 through 0.7.22 say about today's registry.
#[test]
fn an_unknown_cleaner_is_journalled_as_a_refusal() {
    let mut data = accepted_registry();
    data["targets"][0]["disk_cleanup"]["cleaners"]["chromium_profiles"] =
        json!({ "min_age_seconds": 86400 });

    let exc = resolve_canonical_policy(&data, HOST).expect_err("an unknown cleaner is refused");
    assert!(
        exc.message.contains("unknown cleaners"),
        "the private detail is the rejection this test means to provoke: {}",
        exc.message
    );
    assert_eq!(exc.error_code(), "NotImplementedError");
    assert_eq!(journal_entry(&exc), "policy:NotImplementedError");
}

/// Window two's shape: the required field set moved, so the document declares
/// a `disk_cleanup` this build does not recognize as complete. This is what
/// 0.13.24 says about today's registry. A different message, the same class,
/// and it must reach the journal as the same code.
#[test]
fn a_changed_required_field_set_is_journalled_as_a_refusal() {
    let mut data = accepted_registry();
    data["targets"][0]["disk_cleanup"]["cleanup_after_pass"] = json!(true);

    let exc =
        resolve_canonical_policy(&data, HOST).expect_err("an unknown policy field is refused");
    assert!(
        exc.message.contains("must contain exactly"),
        "the private detail is the field-set rejection: {}",
        exc.message
    );
    assert_eq!(exc.error_code(), "NotImplementedError");
    assert_eq!(journal_entry(&exc), "policy:NotImplementedError");
}

/// A document that does not parse is a different failure and keeps the entry
/// it has always had. This is the exact conversion `fetch_canonical_registry`
/// applies to `serde_json::from_str` on the canonical text.
#[test]
fn a_malformed_document_is_still_journalled_as_a_value_error() {
    let broken = serde_json::from_str::<Value>("{\"schema_version\": 2, \"targets\": [")
        .expect_err("truncated json does not parse");
    let exc = JanitorError::from(broken);

    assert_eq!(exc.error_code(), "ValueError");
    assert_eq!(journal_entry(&exc), "policy:ValueError");
}

/// The two are distinguishable, which is the whole point: an operator reading
/// the journal can tell "this document is broken" from "this build is too old
/// to accept it" without a second source.
#[test]
fn a_refusal_and_a_parse_failure_do_not_share_an_entry() {
    let mut data = accepted_registry();
    data["targets"][0]["disk_cleanup"]["cleaners"]["chromium_profiles"] =
        json!({ "min_age_seconds": 86400 });
    let refused = resolve_canonical_policy(&data, HOST).expect_err("refused");
    let malformed =
        JanitorError::from(serde_json::from_str::<Value>("not json").expect_err("does not parse"));

    assert_ne!(journal_entry(&refused), journal_entry(&malformed));
}

/// The entry stays `<stage>:<code>`. The rejection sentence names field paths
/// and declared values; `error_code` exists so none of that is recorded, and
/// the refusal code must not become an exception to that.
#[test]
fn the_refusal_entry_carries_no_paths_values_or_versions() {
    let mut data = accepted_registry();
    data["targets"][0]["disk_cleanup"]["cleaners"]["chromium_profiles"] =
        json!({ "min_age_seconds": 86400 });
    let exc = resolve_canonical_policy(&data, HOST).expect_err("refused");
    let entry = journal_entry(&exc);

    assert_eq!(entry, "policy:NotImplementedError");
    for leak in [
        "chromium_profiles",
        "registry.targets",
        "disk_cleanup",
        env!("CARGO_PKG_VERSION"),
    ] {
        assert!(
            !entry.contains(leak),
            "the journal entry must not carry {leak}: {entry}"
        );
    }
}
