//! What must never happen, and what must happen instead when the sign-in loop
//! is broken somewhere else entirely.

use crate::fixture::{finding, stdout, Fixture, LOCKED_ITEM, STORED_SEED};
use crate::journal::{attempt, CODE_REFUSED, FAILED, RUNTIME_BROKEN};

const AN_HOUR: i64 = 3600;
const A_DAY: i64 = 86_400;

/// The failure mode that made this diagnostic necessary, in reverse: a Weles
/// runtime crash-looping on `ERR_MODULE_NOT_FOUND` fails every reauth forever
/// and looks exactly like a stale seed from outside. Reporting it as one would
/// send an operator to re-enrol Google while the real repair is a release.
#[test]
fn failures_before_the_authenticator_step_are_not_a_stale_seed() {
    let host = Fixture::new();
    host.store_login(LOCKED_ITEM, STORED_SEED);
    crate::journal::write(
        &host.journal(),
        &[
            attempt(LOCKED_ITEM, A_DAY, FAILED, RUNTIME_BROKEN),
            attempt(LOCKED_ITEM, AN_HOUR, FAILED, RUNTIME_BROKEN),
        ],
    );

    let (report, output) = host.freshness(None);

    let row = finding(&report, LOCKED_ITEM);
    assert_eq!(
        row["verdict"],
        serde_json::json!("seed_present_failing_elsewhere"),
        "no code was ever submitted, so the seed is not what these failures are about: {row:#}"
    );
    assert_eq!(row["needs_reenrolment"], serde_json::json!(false));
    assert_eq!(row["code_submitting_attempts"], serde_json::json!(0));
    assert_eq!(row["attempts_recorded"], serde_json::json!(2));
    let repair = row["repair"]
        .as_str()
        .expect("the row still says what to do");
    assert!(
        repair.contains("failing before the authenticator step"),
        "{repair}"
    );
    assert!(
        !repair.contains("store-login-totp-seed.sh"),
        "storing a new seed repairs nothing here: {repair}"
    );
    assert!(output.status.success());
}

/// The safety property. A report that leaked a seed, a password or a live code
/// would be worse than no diagnostic — and the vault here really holds one,
/// while the journal's `detail` carries the seed, a code and a password the
/// way a real trajectory tail carries rendered page text. The host-side reader
/// is what strips it: only marker names may cross.
#[test]
fn no_secret_material_reaches_the_report() {
    let host = Fixture::new();
    host.store_login(LOCKED_ITEM, STORED_SEED);
    let leaky = format!("{CODE_REFUSED} secret={STORED_SEED} code=123456 password=hunter2");
    crate::journal::write(
        &host.journal(),
        &[attempt(LOCKED_ITEM, AN_HOUR, FAILED, &leaky)],
    );

    let (report, output) = host.freshness(None);

    // The finding is still made, so this is not passing by reporting nothing.
    assert_eq!(
        finding(&report, LOCKED_ITEM)["verdict"],
        serde_json::json!("seed_rejected_since")
    );
    assert!(finding(&report, LOCKED_ITEM)["markers"]
        .as_array()
        .expect("the markers travel")
        .contains(&serde_json::json!("google_said_wrong_code")));
    let rendered = serde_json::to_string(&report).expect("the report serialises");
    let console = stdout(&host.freshness_lines());
    for forbidden in [STORED_SEED, "123456", "hunter2", "password="] {
        assert!(
            !rendered.contains(forbidden),
            "the report must never carry {forbidden}: {rendered}"
        );
        assert!(
            !console.contains(forbidden),
            "and neither may the console: {console}"
        );
    }
    assert!(output.status.success());
}
