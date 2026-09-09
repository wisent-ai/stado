//! What `stado disk-cleanup --once` journals about a registry it will not act
//! on, measured on the machine running the test.

use serde_json::json;

use crate::fixture::Journey;

/// Window two's shape: the declared `disk_cleanup` carries a field this build
/// does not implement, so the whole policy is refused.
///
/// The entry is the operator's only signal, and it must not read like a broken
/// document. The refused pass is also a pass that deleted nothing: the
/// backdated cache an enforcing pass would have removed is still on disk when
/// the command exits, and the verdict is still there in the janitor's state
/// file afterwards.
#[test]
fn a_policy_field_this_build_does_not_implement_is_journalled_as_a_refusal() {
    let journey = Journey::new();
    let mut policy = journey.policy();
    policy["cleanup_after_pass"] = json!(true);
    journey.declare(policy);
    let candidate = journey.eligible_cache("refused-candidate");

    let report = journey.cleanup(&["disk-cleanup", "--once"]);

    assert_eq!(
        report["errors"],
        json!(["policy:NotImplementedError"]),
        "refused pass: {report:#}"
    );
    assert_eq!(report["outcome"], "invalid_or_unavailable_policy");
    assert_eq!(
        report["cleaners"],
        json!(null),
        "a refused pass has no scan"
    );
    assert!(
        candidate.join("payload.bin").is_file(),
        "a refused policy must authorize no deletion"
    );
    let state = journey.persisted();
    assert_eq!(
        state["report"]["errors"],
        json!(["policy:NotImplementedError"]),
        "persisted state: {state:#}"
    );
    assert!(
        journey
            .reported_janitor_line()
            .starts_with("janitor: invalid_or_unavailable_policy"),
        "space report: {}",
        journey.reported_janitor_line()
    );
}

/// A document that does not parse is a different failure and keeps the entry
/// it has always had, so the two remain distinguishable without a second
/// source.
#[test]
fn a_registry_document_that_does_not_parse_is_journalled_as_a_value_error() {
    let journey = Journey::new();
    journey.write_registry("{\"schema_version\": 2, \"targets\": [");
    let candidate = journey.eligible_cache("unparsed-candidate");

    let report = journey.cleanup(&["disk-cleanup", "--once"]);

    assert_eq!(
        report["errors"],
        json!(["policy:ValueError"]),
        "malformed pass: {report:#}"
    );
    assert_eq!(report["outcome"], "invalid_or_unavailable_policy");
    assert!(
        candidate.join("payload.bin").is_file(),
        "an unreadable registry must authorize no deletion"
    );
    assert_eq!(
        journey.persisted()["report"]["errors"],
        json!(["policy:ValueError"])
    );
}

/// The rejection sentence names field paths and declared values; `error_code`
/// exists so none of that is recorded. The persisted entry is what an operator
/// and every later reader see, so the leak check belongs on the state file.
#[test]
fn the_journalled_entry_carries_no_field_paths_values_or_versions() {
    let journey = Journey::new();
    let mut policy = journey.policy();
    policy["cleanup_after_pass"] = json!(true);
    journey.declare(policy);

    journey.cleanup(&["disk-cleanup", "--once"]);

    let state = journey.persisted();
    let entry = state["report"]["errors"][0]
        .as_str()
        .unwrap_or_else(|| panic!("no journal entry in {state:#}"))
        .to_string();
    assert_eq!(entry, "policy:NotImplementedError");
    for leak in [
        "cleanup_after_pass",
        "registry.targets",
        "disk_cleanup",
        "must contain exactly",
        env!("CARGO_PKG_VERSION"),
    ] {
        assert!(
            !entry.contains(leak),
            "the journal entry must not carry {leak}: {entry}"
        );
    }
}

/// An unknown cleaner NAME is not a refusal. It is skipped, named in the
/// report, and every cleaner this build does know keeps running — which is the
/// whole point: one registry document is read by every release in the fleet at
/// once, and the older readers must not stop cleaning the moment a newer
/// cleaner is declared for a binary that has not landed yet.
#[test]
fn an_unknown_cleaner_name_is_skipped_and_the_declared_cleaner_runs() {
    let journey = Journey::new();
    let mut policy = journey.policy();
    policy["cleaners"]["chromium_profiles"] = json!({"min_age_seconds": 86400});
    journey.declare(policy);
    let candidate = journey.eligible_cache("declared-candidate");

    let report = journey.cleanup(&["disk-cleanup", "--once"]);

    assert_eq!(report["errors"], json!([]), "skipping pass: {report:#}");
    assert_eq!(report["unknown_cleaners"], json!(["chromium_profiles"]));
    assert_eq!(report["outcome"], "reclaimed_progress");
    assert_eq!(report["cleaners"]["build_caches"]["deleted_items"], 1);
    assert!(
        !candidate.exists(),
        "the cleaner this build implements must still delete its eligible cache"
    );
    assert_eq!(
        journey.persisted()["report"]["unknown_cleaners"],
        json!(["chromium_profiles"])
    );
}
