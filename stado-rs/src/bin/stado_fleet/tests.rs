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

mod ops {
    use serde_json::json;

    use crate::fleet::{find_fleet, parse_fleets};
    use crate::ops::{assign_target, create_fleet, preflight_enroll};

    fn base() -> serde_json::Value {
        json!({
            "fleets": [{ "name": "core", "notes": "always on" }],
            "targets": [{ "name": "mini", "fleet": "core" }, { "name": "laptop" }]
        })
    }

    #[test]
    fn create_appends_entry_and_preserves_existing() {
        let next = create_fleet(&base(), "lab", "experiments").expect("create");
        let fleets = parse_fleets(&next).expect("parse");
        let lab = find_fleet(&fleets, "lab").expect("lab fleet");
        assert_eq!(lab.notes, "experiments");
        assert!(lab.members.is_empty());
        assert!(find_fleet(&fleets, "core").is_some());
    }

    #[test]
    fn create_refuses_duplicate() {
        let err = create_fleet(&base(), "core", "again").unwrap_err();
        assert!(err.contains("already exists"), "unexpected error: {err}");
    }

    #[test]
    fn create_refuses_malformed_name() {
        let err = create_fleet(&base(), "Not A Fleet", "").unwrap_err();
        assert!(
            err.contains("lowercase fleet identifier"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn assign_sets_membership_on_existing_target() {
        let next = assign_target(&base(), "laptop", "core").expect("assign");
        let fleets = parse_fleets(&next).expect("parse");
        let core = find_fleet(&fleets, "core").expect("core fleet");
        assert_eq!(
            core.members,
            vec!["mini".to_string(), "laptop".to_string()]
        );
    }

    #[test]
    fn assign_refuses_undeclared_fleet() {
        let err = assign_target(&base(), "laptop", "ghost").unwrap_err();
        assert!(err.contains("not declared"), "unexpected error: {err}");
    }

    #[test]
    fn assign_refuses_unknown_target() {
        let err = assign_target(&base(), "ghost", "core").unwrap_err();
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    #[test]
    fn reassign_moves_target_between_fleets() {
        let with_lab = create_fleet(&base(), "lab", "experiments").expect("create");
        let moved = assign_target(&with_lab, "mini", "lab").expect("reassign");
        let fleets = parse_fleets(&moved).expect("parse");
        let core = find_fleet(&fleets, "core").expect("core fleet");
        assert!(core.members.is_empty());
        let lab = find_fleet(&fleets, "lab").expect("lab fleet");
        assert_eq!(lab.members, vec!["mini".to_string()]);
    }

    #[test]
    fn enroll_preflight_refuses_registered_target() {
        let err = preflight_enroll(&base(), "mini", None).unwrap_err();
        assert!(err.contains("already registered"), "unexpected error: {err}");
    }

    #[test]
    fn enroll_preflight_refuses_undeclared_fleet() {
        let err = preflight_enroll(&base(), "new-box", Some("ghost")).unwrap_err();
        assert!(err.contains("not declared"), "unexpected error: {err}");
    }

    #[test]
    fn enroll_preflight_accepts_new_machine_with_fleet() {
        preflight_enroll(&base(), "new-box", Some("core")).expect("preflight");
    }

    #[test]
    fn enroll_preflight_accepts_new_machine_without_fleet() {
        preflight_enroll(&base(), "new-box", None).expect("preflight");
    }
}
