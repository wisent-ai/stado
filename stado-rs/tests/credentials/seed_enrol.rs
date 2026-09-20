// The two decisions `stado credentials seed-enrol` takes before it contacts
// anything, attached to src/cli/seed_enrol/mod.rs.
//
// The command's whole effect happens on another machine: it hands one item id
// to that host's Weles worker and then believes the vault about what landed.
// So the id it forwards must be an id, and the vault's answer must be read the
// same whether one row was asked for or all of them — a misread answer would
// report a seed as enrolled while the row is still empty, which is the exact
// class of false verdict this command exists to end.

use super::*;
use crate::cli::seed_freshness::states::rows_of;
use serde_json::json;

#[test]
fn an_item_id_that_is_not_one_is_refused_rather_than_forwarded() {
    assert_eq!(
        checked_login_item("  codex-wisent-google-sso  ").expect("a plain id"),
        "codex-wisent-google-sso"
    );
    assert_eq!(
        checked_login_item("25277710-3398-4273-a1a0-97c776b542ad").expect("a uuid id"),
        "25277710-3398-4273-a1a0-97c776b542ad"
    );
    for refused in [
        "",
        "   ",
        "codex; rm -rf /",
        "codex sso",
        "codex/../secret",
        "codex\nsso",
    ] {
        let error = checked_login_item(refused).expect_err("must be refused");
        assert!(
            format!("{error}").contains("is not a Skarbiec item id"),
            "{refused:?} was accepted or refused for the wrong reason"
        );
    }
}

#[test]
fn one_row_and_a_whole_sweep_read_as_the_same_list() {
    // Skarbiec answers a named item as one object and a sweep as `rows`.
    let single = json!({
        "item": "codex-wisent-google-sso",
        "kind": "login",
        "seed_state": "declared_empty"
    });
    assert_eq!(
        rows_of(&single),
        vec![(
            "codex-wisent-google-sso".to_string(),
            "declared_empty".to_string()
        )]
    );

    let sweep = json!({"rows": [
        {"item": "a", "seed_state": "present"},
        {"item": "b", "seed_state": "field_absent"},
    ]});
    assert_eq!(
        rows_of(&sweep),
        vec![
            ("a".to_string(), "present".to_string()),
            ("b".to_string(), "field_absent".to_string()),
        ]
    );

    // A row that names no state is dropped rather than read as a state of its
    // own: an unreadable row must never count as a seed.
    let partial = json!({"rows": [{"item": "a"}, {"seed_state": "present"}]});
    assert!(rows_of(&partial).is_empty());
}
