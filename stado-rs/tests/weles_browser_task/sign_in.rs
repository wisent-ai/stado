//! Signing in: refused before a capability exists, because a minted one is
//! single-use and would be spent finding out the plan was wrong.
//!
//! A sign-in needs both halves — the origin whose fields are filled and the
//! vault item holding the account — plus the caller's word that the run may
//! log in at all. Handing an agent credentials while its own instructions say
//! "do not log in" is two orders, and that is the one mechanical consequence
//! `allow_login` has.

use serde_json::json;

use crate::fixture::{refusal, said, Fleet, DEFAULT_ACTION, TARGET};

const ITEM: &str = "weles-google-sso-login";
const ORIGIN: &str = "https://accounts.google.com";

fn declared_fleet() -> Fleet {
    let fleet = Fleet::declaring(&[DEFAULT_ACTION]);
    fleet.allowlist(&format!("{DEFAULT_ACTION}\n"));
    fleet
}

#[test]
fn half_a_sign_in_is_refused_naming_the_missing_half() {
    let fleet = declared_fleet();

    let plan = fleet.plan(json!({ "allow_login": true, "sign_in_origin": ORIGIN }));
    let out = fleet.run(Some(TARGET), &plan);
    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        "weles-browser-task plan sign_in_origin needs sign_in_item; add the vault item"
    );

    let plan = fleet.plan(json!({ "allow_login": true, "sign_in_item": ITEM }));
    let out = fleet.run(Some(TARGET), &plan);
    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        "weles-browser-task plan sign_in_item needs sign_in_origin; add the page origin"
    );
}

#[test]
fn a_sign_in_the_run_never_asked_for_is_refused() {
    let fleet = declared_fleet();
    let plan = fleet.plan(json!({ "sign_in_origin": ORIGIN, "sign_in_item": ITEM }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        "weles-browser-task plan sign_in_origin requires allow_login=true"
    );
}

/// Weles builds its expectation from the live page's `origin`, so an origin
/// carrying a path could never match it.
///
/// The refusal must name the thing the operator can actually edit. There is no
/// `--sign-in-origin` flag on any command in this build: the only surface is
/// this plan's `sign_in_origin` field, and every other refusal in this runner
/// says so. A sentence naming a flag nobody can type sends the operator
/// looking for it.
#[test]
fn an_origin_weles_could_never_match_is_refused_in_the_plans_own_words() {
    let fleet = declared_fleet();
    let plan = fleet.plan(json!({
        "allow_login": true,
        "sign_in_origin": "https://accounts.google.com/signin/v2",
        "sign_in_item": ITEM,
    }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        "weles-browser-task plan sign_in_origin must be a bare origin such as \
         https://accounts.google.com, with no path, query or fragment: \
         https://accounts.google.com/signin/v2"
    );
}

#[test]
fn an_origin_no_browser_could_fill_is_refused_in_the_workers_own_sentence() {
    let fleet = declared_fleet();
    let plan = fleet.plan(json!({
        "allow_login": true,
        "sign_in_origin": "ftp://accounts.google.com",
        "sign_in_item": ITEM,
    }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(refusal(&out), "credential fill requires an HTTP(S) origin");
}

/// The capability must exist in the broker the WORKER talks to, so it is
/// issued on the target. The registry row here is this machine and the home is
/// a tempdir that genuinely holds no `.stado/bin/skarbiec` — nothing is
/// stubbed. The refusal has to name the host and the path rather than submit a
/// run whose prefill could never be redeemed.
#[test]
fn a_sign_in_is_refused_when_the_target_carries_no_capability_broker() {
    let fleet = declared_fleet();
    let plan = fleet.plan(json!({
        "allow_login": true,
        "sign_in_origin": ORIGIN,
        "sign_in_item": ITEM,
    }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        format!(
            "{TARGET}: no Skarbiec binary at {}/.stado/bin/skarbiec; install Skarbiec at that \
             path on the declared active host",
            fleet.home().display()
        )
    );
    assert!(
        fleet.files_naming_the_session().is_empty(),
        "a refused sign-in recorded the session: {:?}",
        fleet.files_naming_the_session()
    );
}
