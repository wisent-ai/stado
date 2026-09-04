//! A vault that could not answer is not an authorization verdict.
//!
//! # The incident
//!
//! `object-auth` reads every mapped item through the four verifier grants, and
//! it turned every failure of that read into one sentence: "authorization
//! fails closed because mapping, verifier grant, or mapped token validation
//! failed", published with `error_code=auth`.
//!
//! On 2026-09-04 the vault on `lukasz-macbook` answered
//! `503 {"error":"item is stored but could not be decrypted","error_code":"infra_down"}`
//! because its `keyboxd` was wedged — for items whose secret keys were in the
//! keyring and whose grants were exactly right. The row read FAIL, blocked the
//! deployment preflight of a release train, and then read PASS again on the
//! next sweep once a fresh `gpg` restarted the daemon. A row that alternates
//! between "the deployment is broken" and "the deployment is fine", for a
//! deployment that did not change, teaches an operator to discount the table —
//! which is the failure `Status::Unmeasured` exists to prevent.
//!
//! # What is defended
//!
//! * four readable verifiers is a PASS, and says what it counted;
//! * a vault that returns 5xx, or cannot be reached at all, is UNMEASURED —
//!   the check states that it measured nothing rather than inventing a verdict;
//! * a real configuration verdict still FAILS, and any unavailable verifier
//!   beside it is named so the reader knows which half was measured;
//! * the distinction is typed on the error, not recovered by matching words in
//!   a message.

use stado::doctor::{object_auth_verdict, Status};
use stado::skarbiec::SkarbiecError;

fn unavailable(status: u16) -> SkarbiecError {
    SkarbiecError::Response {
        status,
        detail:
            r#"{"error":"item is stored but could not be decrypted","error_code":"infra_down"}"#
                .to_string(),
    }
}

fn misconfigured() -> SkarbiecError {
    SkarbiecError::Deployment(
        "object verifier grant item set mismatch (missing=[spis-crawls-object-api], unexpected=[])"
            .to_string(),
    )
}

#[test]
fn four_readable_verifiers_pass_and_say_what_they_counted() {
    let check = object_auth_verdict(Ok(18), Ok(16), Ok(2), Ok(4));
    assert_eq!(check.status, Status::Pass);
    assert!(
        check.detail.contains("18 namespace items"),
        "{}",
        check.detail
    );
    assert!(
        check.detail.contains("4 deployer items"),
        "{}",
        check.detail
    );
}

#[test]
fn a_vault_that_could_not_answer_is_not_measured() {
    let check = object_auth_verdict(Err(unavailable(503)), Ok(16), Ok(2), Ok(4));
    assert_eq!(check.status, Status::Unmeasured);
    assert!(
        check.detail.starts_with("not measured:"),
        "{}",
        check.detail
    );
    assert!(
        !check.detail.contains("fails closed"),
        "an unreadable vault must not be reported as an authorization failure: {}",
        check.detail
    );
    assert!(check.detail.contains("infra_down"), "{}", check.detail);
}

#[test]
fn every_server_side_status_and_no_client_status_counts_as_unavailable() {
    for status in [500, 502, 503, 504] {
        assert!(
            unavailable(status).is_unavailable(),
            "HTTP {status} is the vault saying it cannot answer"
        );
    }
    for status in [400, 401, 403, 404, 409] {
        assert!(
            !SkarbiecError::Response {
                status,
                detail: r#"{"error":"consumer not authorized to read item field"}"#.to_string(),
            }
            .is_unavailable(),
            "HTTP {status} is an answer about authorization, not an outage"
        );
    }
    assert!(!misconfigured().is_unavailable());
}

#[test]
fn a_configuration_verdict_still_fails_and_names_the_unavailable_half() {
    let check = object_auth_verdict(Err(misconfigured()), Err(unavailable(503)), Ok(2), Ok(4));
    assert_eq!(check.status, Status::Fail);
    assert!(check.detail.contains("fails closed"), "{}", check.detail);
    assert!(
        check.detail.contains("spis-crawls-object-api"),
        "the measured half keeps its exact sentence: {}",
        check.detail
    );
    assert!(
        check.detail.contains("release verifier"),
        "the unmeasured half is still named: {}",
        check.detail
    );
}
