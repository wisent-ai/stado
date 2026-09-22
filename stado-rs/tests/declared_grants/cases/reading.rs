//! What `service grants` reads back, and what it refuses to read.

use crate::fixture::{stderr, stdout, untouched, Store};

#[test]
fn a_declared_grant_is_printed_with_what_minting_would_use() {
    let store = Store::new();
    let out = store.grants(&["brama"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let said = stdout(&out);
    assert!(said.contains("brama on w1:"), "{said}");
    assert!(
        said.contains("declared grant(s); nothing was minted"),
        "{said}"
    );
    assert!(said.contains("oko-model-router-client"), "{said}");
    assert!(said.contains("read:oko-model-router#token"), "{said}");
    assert!(said.contains("oko-model-router-skarbiec-token"), "{said}");
    assert!(
        said.contains("stado service grants brama --apply"),
        "the plan does not name what mints it: {said}"
    );
    untouched(store.home.path());
}

#[test]
fn the_declaration_is_readable_as_json_and_says_it_was_not_applied() {
    let store = Store::new();
    let out = store.grants(&["brama", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&stdout(&out)).expect("the plan prints JSON");
    assert_eq!(rows.len(), REGISTRY.matches("\"token_file\"").count());
    assert_eq!(rows[0]["authorized"], "oko");
    assert_eq!(rows[0]["consumer"], "oko-model-router-client");
    assert_eq!(
        rows[0]["audience"], "oko-model-router-client",
        "the audience defaults to the consumer"
    );
    assert_eq!(rows[0]["host"], "w1");
    assert_eq!(rows[0]["applied"], false);
}

#[test]
fn a_service_with_no_declaration_is_refused_with_where_one_goes() {
    let store = Store::new();
    let out = store.grants(&["kronika"]);
    assert!(!out.status.success());
    let said = stderr(&out);
    assert!(
        said.contains("no consumer of kronika declares a grant"),
        "{said}"
    );
    assert!(
        said.contains("service_directory.services.kronika.consumers.<consumer>.grants"),
        "the refusal does not say where a declaration goes: {said}"
    );
    assert!(said.contains("stado registry set --path"), "{said}");
}

#[test]
fn an_unknown_service_and_an_unauthorized_consumer_are_refused_with_the_names() {
    let store = Store::new();
    let out = store.grants(&["nope"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("carries no service \"nope\"; services there: brama, kronika"),
        "{}",
        stderr(&out)
    );

    let out = store.grants(&["brama", "--consumer", "weles"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(
            "brama does not authorize consumer \"weles\"; consumers there: oko, operator"
        ),
        "{}",
        stderr(&out)
    );
}

/// A host whose Stado predates the field parses the directory strictly and
/// would resolve nothing at all once a consumer carries it. On 2026-09-20 the
/// host every service resolves through ran 0.21.32 while the field arrived in
