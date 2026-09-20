//! `classify_message` over the wording this fleet actually gets back.
//!
//! The families live in `src/primitives/failure-needles.json`, and the order
//! they are tested in is the load-bearing part: `find: /x: Permission denied`
//! is an unreadable file, not a rejected credential, while `Permission denied
//! (publickey)` is a rejected credential however it is spelled around a path.
//! On 2026-09-17 `stado space report lukasz-macbook` told the operator to
//! check their credentials over one unreadable Google Drive `.tmp`.
//!
//! Every case below is the real classifier, called the way the CLI calls it.

use stado::primitives::failure::{classify_message, FailureCode};

#[test]
fn an_unset_variable_is_our_configuration_not_a_missing_resource() {
    assert_eq!(
        classify_message("WC_BUCKET is required"),
        FailureCode::Config
    );
    assert_eq!(
        classify_message("gcloud: command not found"),
        FailureCode::Config,
    );
}

#[test]
fn ssh_refusing_our_key_is_a_credential_failure() {
    assert_eq!(
        classify_message("git@github.com: Permission denied (publickey)."),
        FailureCode::Auth,
    );
}

#[test]
fn a_file_the_host_would_not_open_is_not_a_credential_failure() {
    assert_eq!(
        classify_message("find: /Users/x/Google Drive/.tmp: Permission denied"),
        FailureCode::Unknown,
    );
    assert_eq!(
        classify_message("failed to read report.json (os error 13)"),
        FailureCode::Unknown,
    );
}

#[test]
fn a_rejected_credential_is_reported_as_one() {
    assert_eq!(
        classify_message("authentication failed for service account"),
        FailureCode::Auth,
    );
}

#[test]
fn the_families_each_answer_with_their_own_code() {
    assert_eq!(
        classify_message("quota exceeded, retry after 30s"),
        FailureCode::RateLimit,
    );
    assert_eq!(
        classify_message("operation timed out after deadline exceeded"),
        FailureCode::Timeout,
    );
    assert_eq!(
        classify_message("tcp connect error: connection refused"),
        FailureCode::InfraDown,
    );
    assert_eq!(
        classify_message("blob not found: queue/1a2b3c4d.json"),
        FailureCode::NotFound,
    );
}

#[test]
fn an_http_status_in_the_prose_beats_the_wording() {
    // The sentence says "not found"; the status says the backend is down.
    assert_eq!(
        classify_message("GCS API error HTTP 503: backend not found"),
        FailureCode::InfraDown,
    );
    assert_eq!(
        classify_message("HTTP 429: slow down"),
        FailureCode::RateLimit
    );
}

#[test]
fn wording_nobody_has_seen_yet_stays_unknown() {
    assert_eq!(
        classify_message("the disk fell over in a way nobody wrote a needle for"),
        FailureCode::Unknown,
    );
}
