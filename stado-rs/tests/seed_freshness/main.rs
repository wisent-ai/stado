//! `stado host authenticator-seed-freshness` — the four conditions it must
//! never collapse into one another.
//!
//! Nothing in this fleet could answer "is the stored authenticator seed still
//! the one this account has enrolled". The only signal was a login failing,
//! weeks later, in a loop nobody read: on charless-mac-mini Brama drove a
//! browser sign-in for three providers every thirty minutes for six days,
//! resubmitting a code two Google accounts had already rejected, until Google
//! answered "Too many failed attempts" and locked the authenticator method —
//! destroying the operator's own ability to repair it by hand.
//!
//! The check joins the vault's half (does a seed exist) with the recorded
//! sign-in history's half (were codes from it accepted, and since when were
//! they refused). What is defended here is exactly the discrimination, because
//! a verdict that cannot tell these apart names the wrong repair:
//!
//! * a seed whose codes were accepted is `seed_last_known_good` and carries no
//!   repair;
//! * a seed refused on every attempt since an instant is
//!   `seed_rejected_since`, names that instant, and names the exact command
//!   that stores a new seed;
//! * a `totp_secret` field declared and carrying nothing is `seed_field_empty`
//!   — the condition earlier probing saw as `has_seed: false` on accounts that
//!   declare the field;
//! * a row whose kind has no `totp_secret` field at all is `seed_field_absent`
//!   and its repair is not "store a seed";
//! * sign-ins failing before the authenticator step — a crash-looping Weles
//!   runtime, an unreachable worker — are `seed_present_failing_elsewhere` and
//!   must NEVER be reported as a stale seed, because the repair is to fix the
//!   release, not to re-enrol;
//! * an accepted code AFTER a run of refusals means the seed was replaced, so
//!   the row is good again and the streak is over.
//!
//! And the safety property the whole design turns on: no seed, password or
//! one-time code appears anywhere in the report, and the host-side reader
//! carries out marker names rather than the journal's raw `detail`, which
//! holds up to 1800 characters of rendered page text.
//!
//! These are library-level tests: `classify` and `build_report` are the
//! discriminator, and they take the two host answers as data, so the whole
//! verdict table is exercised without a host, a vault or a browser.

use serde_json::{json, Value};

use stado::cli::seed_freshness::{
    attempts_of, build_report, classify, Attempt, Verdict, SEED_DECLARED_EMPTY, SEED_FIELD_ABSENT,
    SEED_PRESENT, SEED_UNREADABLE,
};

/// The account the six-day loop locked out.
const LOCKED_ITEM: &str = "codex-wisent-google-sso";

/// One recorded attempt. `at_ms` is derived from the instant so a test cannot
/// accidentally order its own evidence differently from the way the host does.
fn attempt(at: &str, result: &str, markers: &[&str]) -> Attempt {
    let has = |name: &str| markers.contains(&name);
    let rejected = has("authenticator_wrong_code_after_retries")
        || has("google_said_wrong_code")
        || has("google_said_too_many_failed_attempts");
    let submitted = has("code_submitted") || rejected;
    Attempt {
        at: at.to_string(),
        at_ms: chrono::DateTime::parse_from_rfc3339(at)
            .expect("test instants must be RFC 3339")
            .timestamp_millis(),
        result: result.to_string(),
        code_submitted: submitted,
        code_rejected: rejected,
        locked_out: has("google_said_too_many_failed_attempts"),
        authenticator_unreached: !submitted
            && (has("authenticator_code_input_missing")
                || has("authenticator_option_not_clickable")
                || has("authenticator_method_not_reached")),
        markers: markers.iter().map(|name| name.to_string()).collect(),
    }
}

/// A seed whose code the provider accepted is good, and says when.
#[test]
fn an_accepted_code_is_last_known_good_and_names_the_instant() {
    let verdict = classify(
        SEED_PRESENT,
        &[
            attempt("2026-08-20T10:00:00Z", "signed_in", &["code_submitted"]),
            attempt("2026-08-26T10:00:00Z", "signed_in", &["code_submitted"]),
        ],
    );
    assert_eq!(
        verdict,
        Verdict::LastKnownGood {
            at: "2026-08-26T10:00:00Z".to_string()
        }
    );
    assert!(!verdict.needs_reenrolment());
    assert!(verdict.repair(LOCKED_ITEM).is_empty());
}

/// The condition the fleet could not name: accepted once, refused ever since.
/// The verdict must be the streak's start, not the first attempt ever.
#[test]
fn codes_refused_on_every_attempt_since_a_date_name_that_date() {
    let verdict = classify(
        SEED_PRESENT,
        &[
            attempt("2026-08-20T10:00:00Z", "signed_in", &["code_submitted"]),
            attempt(
                "2026-08-27T12:00:00Z",
                "failed",
                &["code_submitted", "google_said_wrong_code"],
            ),
            attempt(
                "2026-08-28T12:00:00Z",
                "failed",
                &["code_submitted", "authenticator_wrong_code_after_retries"],
            ),
            attempt(
                "2026-09-01T12:00:00Z",
                "failed",
                &["google_said_too_many_failed_attempts"],
            ),
        ],
    );
    assert_eq!(
        verdict,
        Verdict::RejectedSince {
            since: "2026-08-27T12:00:00Z".to_string(),
            attempts: 3,
            locked_out: true,
        }
    );
    assert!(verdict.needs_reenrolment());
}

/// The repair has to be actionable: the exact command, the real login item,
/// and the fact that Google's lockout blocks re-enrolment until it clears.
#[test]
fn the_repair_names_the_exact_command_and_the_account() {
    let verdict = classify(
        SEED_PRESENT,
        &[attempt(
            "2026-09-01T12:00:00Z",
            "failed",
            &["google_said_too_many_failed_attempts"],
        )],
    );
    let repair = verdict.repair(LOCKED_ITEM);
    assert!(repair.contains("store-login-totp-seed.sh"), "{repair}");
    assert!(repair.contains(&format!("ACCOUNT={LOCKED_ITEM}")), "{repair}");
    assert!(repair.contains("re-enrol"), "{repair}");
    assert!(repair.contains("locked the authenticator method"), "{repair}");
}

/// A seed stored after a run of refusals ends the streak. Without this the
/// check would keep telling an operator to re-enrol an account they just
/// repaired.
#[test]
fn an_accepted_code_after_refusals_clears_the_streak() {
    let verdict = classify(
        SEED_PRESENT,
        &[
            attempt(
                "2026-08-27T12:00:00Z",
                "failed",
                &["code_submitted", "google_said_wrong_code"],
            ),
            attempt("2026-09-02T12:00:00Z", "signed_in", &["code_submitted"]),
        ],
    );
    assert_eq!(
        verdict,
        Verdict::LastKnownGood {
            at: "2026-09-02T12:00:00Z".to_string()
        }
    );
}

/// A declared field carrying nothing, and a kind with no such field, are two
/// conditions with two repairs. Neither is "the seed is stale".
#[test]
fn an_empty_field_and_an_absent_field_are_different_conditions() {
    let empty = classify(SEED_DECLARED_EMPTY, &[]);
    assert_eq!(empty, Verdict::FieldEmpty);
    assert!(empty.needs_reenrolment());
    assert!(empty.repair(LOCKED_ITEM).contains("store-login-totp-seed.sh"));

    let absent = classify(SEED_FIELD_ABSENT, &[]);
    assert_eq!(absent, Verdict::FieldAbsent);
    assert!(!absent.needs_reenrolment());
    let repair = absent.repair(LOCKED_ITEM);
    assert!(repair.contains("declares no totp_secret field"), "{repair}");
    assert!(
        !repair.contains("store-login-totp-seed.sh"),
        "storing a seed is refused by the schema for this kind: {repair}"
    );
}

/// The vault state decides these two regardless of how much sign-in history
/// exists, because a row with no usable seed cannot have had a code accepted.
#[test]
fn vault_state_outranks_run_history_when_there_is_no_seed() {
    let history = [attempt(
        "2026-09-01T12:00:00Z",
        "signed_in",
        &["code_submitted"],
    )];
    assert_eq!(classify(SEED_DECLARED_EMPTY, &history), Verdict::FieldEmpty);
    assert_eq!(classify(SEED_FIELD_ABSENT, &history), Verdict::FieldAbsent);
    assert_eq!(
        classify(SEED_UNREADABLE, &history),
        Verdict::VaultRowUnreadable
    );
}

/// The failure mode that made this diagnostic necessary in reverse: a Weles
/// runtime crash-looping on `ERR_MODULE_NOT_FOUND` fails every reauth forever
/// and looks exactly like a stale seed. Reporting it as one would send an
/// operator to re-enrol Google while the real repair is a release.
#[test]
fn failures_before_the_authenticator_step_are_not_a_stale_seed() {
    let verdict = classify(
        SEED_PRESENT,
        &[
            attempt("2026-09-02T20:02:55Z", "failed", &["weles_runtime_broken"]),
            attempt("2026-09-02T19:50:42Z", "failed", &["weles_unreachable"]),
        ],
    );
    assert_eq!(verdict, Verdict::PresentFailingElsewhere { attempts: 2 });
    assert!(!verdict.needs_reenrolment());
    let repair = verdict.repair(LOCKED_ITEM);
    assert!(repair.contains("failing before the authenticator step"), "{repair}");
    assert!(!repair.contains("store-login-totp-seed.sh"), "{repair}");
}

/// A seed nothing has ever exercised is untested, not good. Calling it good
/// is how a seed stored years ago and never used reads as healthy.
#[test]
fn a_seed_no_attempt_ever_exercised_is_untested_not_good() {
    assert_eq!(classify(SEED_PRESENT, &[]), Verdict::PresentUntested);
    assert_eq!(
        classify(
            SEED_PRESENT,
            &[attempt("2026-09-01T12:00:00Z", "signed_in", &[])]
        ),
        Verdict::PresentUntested
    );
}

/// Evidence is attributed per account. The six-day loop drove three providers
/// interleaved, so an attempt against one login item must never decide
/// another's verdict.
#[test]
fn attempts_are_attributed_to_their_own_login_item() {
    let evidence = json!({
        "attempts": [
            {
                "login_item": LOCKED_ITEM,
                "at": "2026-08-27T12:00:00Z",
                "at_ms": 1_756_296_000_000_i64,
                "result": "failed",
                "code_submitted": true,
                "code_rejected": true,
                "locked_out": false,
                "authenticator_unreached": false,
                "markers": ["code_submitted", "google_said_wrong_code"],
            },
            {
                "login_item": "kimi-lukasz-google-sso",
                "at": "2026-08-27T12:30:00Z",
                "at_ms": 1_756_297_800_000_i64,
                "result": "signed_in",
                "code_submitted": true,
                "code_rejected": false,
                "locked_out": false,
                "authenticator_unreached": false,
                "markers": ["code_submitted"],
            }
        ]
    });
    assert_eq!(attempts_of(&evidence, LOCKED_ITEM).len(), 1);
    assert!(matches!(
        classify(SEED_PRESENT, &attempts_of(&evidence, LOCKED_ITEM)),
        Verdict::RejectedSince { .. }
    ));
    assert!(matches!(
        classify(SEED_PRESENT, &attempts_of(&evidence, "kimi-lukasz-google-sso")),
        Verdict::LastKnownGood { .. }
    ));
}

/// The whole report, over the real mix of rows this fleet holds: the joined
/// document has to keep the conditions apart, put what needs repair first, and
/// drop rows that are neither seed-bearing nor seed-declaring so the two that
/// matter are not buried.
#[test]
fn the_report_keeps_the_conditions_apart_and_leads_with_what_needs_repair() {
    let vault = json!({"rows": [
        {"item": "claude-wisent-google-sso", "kind": "login", "seed_state": SEED_PRESENT},
        {"item": LOCKED_ITEM, "kind": "login", "seed_state": SEED_PRESENT},
        {"item": "codex-zuzanna-google-sso", "kind": "login", "seed_state": SEED_DECLARED_EMPTY},
        {"item": "some-api-key", "kind": "api-key", "seed_state": SEED_FIELD_ABSENT},
    ]});
    let evidence = json!({
        "journal": {"path_present": true, "sign_in_records": 1111, "attributed_attempts": 2},
        "reauth_runs_seen": 1111,
        "attempts": [
            {
                "login_item": LOCKED_ITEM,
                "at": "2026-08-27T12:00:00Z",
                "at_ms": 1_756_296_000_000_i64,
                "result": "failed",
                "code_submitted": true,
                "code_rejected": true,
                "locked_out": true,
                "authenticator_unreached": false,
                "markers": ["code_submitted", "google_said_too_many_failed_attempts"],
            },
            {
                "login_item": "claude-wisent-google-sso",
                "at": "2026-08-26T12:00:00Z",
                "at_ms": 1_756_209_600_000_i64,
                "result": "signed_in",
                "code_submitted": true,
                "code_rejected": false,
                "locked_out": false,
                "authenticator_unreached": false,
                "markers": ["code_submitted"],
            }
        ]
    });
    let report = build_report("charless-mac-mini", &vault, &evidence);
    let findings = report["findings"].as_array().expect("findings is a list");

    // The api-key row has no seed field and no history: dropped, not reported.
    assert_eq!(findings.len(), 3, "{findings:#?}");
    assert!(findings
        .iter()
        .all(|finding| finding["login_item"] != json!("some-api-key")));

    // What needs a repair leads.
    assert!(findings[0]["needs_reenrolment"].as_bool().unwrap());
    assert!(findings[1]["needs_reenrolment"].as_bool().unwrap());
    assert!(!findings[2]["needs_reenrolment"].as_bool().unwrap());

    let by_item = |item: &str| -> Value {
        findings
            .iter()
            .find(|finding| finding["login_item"] == json!(item))
            .cloned()
            .unwrap_or_else(|| panic!("no finding for {item}"))
    };
    let locked = by_item(LOCKED_ITEM);
    assert_eq!(locked["verdict"], json!("seed_rejected_since"));
    assert_eq!(locked["rejected_since"], json!("2026-08-27T12:00:00Z"));
    assert_eq!(locked["locked_out"], json!(true));

    let good = by_item("claude-wisent-google-sso");
    assert_eq!(good["verdict"], json!("seed_last_known_good"));
    assert_eq!(good["last_known_good_at"], json!("2026-08-26T12:00:00Z"));
    assert_eq!(good["repair"], Value::Null);

    assert_eq!(
        by_item("codex-zuzanna-google-sso")["verdict"],
        json!("seed_field_empty")
    );
}

/// The safety property. A report that leaked a seed, a password or a live code
/// would be worse than no diagnostic, and the host-side reader deliberately
/// carries marker names instead of the journal's raw `detail`.
#[test]
fn no_secret_material_reaches_the_report() {
    let seed = "JBSWY3DPEHPK3PXP";
    let vault = json!({"rows": [
        {"item": LOCKED_ITEM, "kind": "login", "seed_state": SEED_PRESENT},
    ]});
    // A journal `detail` on this host really does carry trajectory tail and
    // page text; the reader is what strips it. Feed the shape a leaky reader
    // would have produced and prove none of it is echoed.
    let evidence = json!({
        "attempts": [{
            "login_item": LOCKED_ITEM,
            "at": "2026-08-27T12:00:00Z",
            "at_ms": 1_756_296_000_000_i64,
            "result": "failed",
            "code_submitted": true,
            "code_rejected": true,
            "locked_out": true,
            "authenticator_unreached": false,
            "markers": ["code_submitted", "google_said_too_many_failed_attempts"],
            "detail": format!("secret={seed} code=123456 password=hunter2"),
        }]
    });
    let rendered = serde_json::to_string(&build_report("charless-mac-mini", &vault, &evidence))
        .expect("the report serializes");
    for forbidden in [seed, "123456", "hunter2", "password="] {
        assert!(
            !rendered.contains(forbidden),
            "the report must never carry {forbidden}: {rendered}"
        );
    }
}
