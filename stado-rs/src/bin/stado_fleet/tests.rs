//! Unit tests for the stado_fleet doctor pure logic: grant drift and
//! allowlist-entry parsing. Hermetic — string slices only.

use crate::doctor::{grant_drift, parse_secret_field};

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn drift_is_empty_when_grant_matches_config() {
    let expected = strings(&["alpha", "beta"]);
    let visible = strings(&["beta", "alpha"]);
    let (missing, extra) = grant_drift(&expected, &visible);
    assert!(missing.is_empty());
    assert!(extra.is_empty());
}

#[test]
fn missing_items_are_reported_sorted() {
    let expected = strings(&["zeta", "alpha", "beta"]);
    let visible = strings(&["beta"]);
    let (missing, extra) = grant_drift(&expected, &visible);
    assert_eq!(missing, strings(&["alpha", "zeta"]));
    assert!(extra.is_empty());
}

#[test]
fn extra_items_in_grant_are_reported() {
    let expected = strings(&["alpha"]);
    let visible = strings(&["alpha", "stray"]);
    let (missing, extra) = grant_drift(&expected, &visible);
    assert!(missing.is_empty());
    assert_eq!(extra, strings(&["stray"]));
}

#[test]
fn empty_expected_means_every_visible_item_is_extra() {
    let expected: Vec<String> = Vec::new();
    let visible = strings(&["stray"]);
    let (missing, extra) = grant_drift(&expected, &visible);
    assert!(missing.is_empty());
    assert_eq!(extra, strings(&["stray"]));
}

#[test]
fn well_formed_entry_splits_into_item_and_field() {
    assert_eq!(
        parse_secret_field("wisent-backend-scheduler#token"),
        Some(("wisent-backend-scheduler", "token"))
    );
}

#[test]
fn entry_without_hash_is_refused() {
    assert_eq!(parse_secret_field("no-field-here"), None);
}

#[test]
fn empty_halves_are_refused() {
    assert_eq!(parse_secret_field("#token"), None);
    assert_eq!(parse_secret_field("item#"), None);
}

#[test]
fn field_may_contain_further_hashes() {
    assert_eq!(
        parse_secret_field("item#field#extra"),
        Some(("item", "field#extra"))
    );
}

mod fleets {
    use serde_json::json;

    use crate::fleet::{find_fleet, parse_fleets};

    #[test]
    fn document_without_section_has_no_fleets() {
        let doc = json!({ "targets": [] });
        assert!(parse_fleets(&doc).expect("parse").is_empty());
    }

    #[test]
    fn membership_resolves_from_target_field() {
        let doc = json!({
            "fleets": [
                { "name": "core", "notes": "always on" },
                { "name": "burst" }
            ],
            "targets": [
                { "name": "mini", "fleet": "core" },
                { "name": "gpu-box", "fleet": "burst" },
                { "name": "laptop" }
            ]
        });
        let fleets = parse_fleets(&doc).expect("parse");
        let core = find_fleet(&fleets, "core").expect("core fleet");
        assert_eq!(core.members, vec!["mini".to_string()]);
        assert_eq!(core.notes, "always on");
        let burst = find_fleet(&fleets, "burst").expect("burst fleet");
        assert_eq!(burst.members, vec!["gpu-box".to_string()]);
        assert!(burst.notes.is_empty());
    }

    #[test]
    fn duplicate_fleet_names_are_refused() {
        let doc = json!({
            "fleets": [{ "name": "core" }, { "name": "core" }],
            "targets": []
        });
        let err = parse_fleets(&doc).unwrap_err();
        assert!(err.contains("duplicate fleet"), "unexpected error: {err}");
    }

    #[test]
    fn dangling_fleet_reference_is_refused() {
        let doc = json!({
            "fleets": [{ "name": "core" }],
            "targets": [{ "name": "mini", "fleet": "ghost" }]
        });
        let err = parse_fleets(&doc).unwrap_err();
        assert!(err.contains("undeclared fleet"), "unexpected error: {err}");
    }

    #[test]
    fn non_string_fleet_field_is_refused() {
        let doc = json!({
            "fleets": [{ "name": "core" }],
            "targets": [{ "name": "mini", "fleet": true }]
        });
        let err = parse_fleets(&doc).unwrap_err();
        assert!(err.contains("must be a string"), "unexpected error: {err}");
    }

    #[test]
    fn malformed_fleet_name_is_refused() {
        let doc = json!({
            "fleets": [{ "name": "Core Team" }],
            "targets": []
        });
        let err = parse_fleets(&doc).unwrap_err();
        assert!(
            err.contains("lowercase fleet identifier"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn non_array_section_is_refused() {
        let doc = json!({ "fleets": { "name": "core" }, "targets": [] });
        let err = parse_fleets(&doc).unwrap_err();
        assert!(err.contains("must be an array"), "unexpected error: {err}");
    }
}
