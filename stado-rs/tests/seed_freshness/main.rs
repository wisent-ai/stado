//! `stado credentials seed-freshness --host TARGET` — is each login row's
//! stored authenticator seed still one its account accepts, and is what the
//! host recorded fresh enough to act on?
//!
//! # What happened
//!
//! Nothing in this fleet could answer that question. The only signal was a
//! login failing, weeks later, in a loop nobody read: on charless-mac-mini
//! Brama drove a browser sign-in for three providers every thirty minutes for
//! six days, resubmitting a code two Google accounts had already rejected,
//! until Google answered "Too many failed attempts" and locked the
//! authenticator method — destroying the operator's own ability to repair it
//! by hand.
//!
//! # What is defended here, and through what
//!
//! The command joins two host reads: the vault's half (does a seed exist),
//! answered by the released Skarbiec on the host, and the recorded sign-in
//! history's half (were codes from it accepted, and since when were they
//! refused), read out of Brama's journal by a program the command sends to the
//! host. Every case here drives the built binary against this machine with
//! both halves real: a Skarbiec vault this fixture creates and fills through
//! the released Skarbiec, and a journal it writes at instants it chose. The
//! verdicts are then read back out of the command's own report, and the
//! instant a verdict names is compared against the instant the case wrote.
//!
//! A verdict that cannot tell these apart names the wrong repair, so the
//! discrimination is what is asserted:
//!
//! * a seed whose codes were accepted is `seed_last_known_good` and carries no
//!   repair;
//! * a seed refused on every attempt since an instant is
//!   `seed_rejected_since`, names that instant, and names the exact command
//!   that stores a new seed;
//! * a `totp_secret` field declared and carrying nothing is `seed_field_empty`
//!   — not a stale seed;
//! * sign-ins failing before the authenticator step — a crash-looping Weles
//!   runtime — are `seed_present_failing_elsewhere` and must NEVER be reported
//!   as a stale seed, because the repair is to fix the release;
//! * an accepted code AFTER a run of refusals means the seed was replaced, so
//!   the row is good again and the streak is over;
//! * a seed nothing has exercised is untested, not good.
//!
//! And the safety property the whole design turns on: no seed, password or
//! one-time code appears anywhere in the report, which is now checked against
//! a report produced from a vault that really holds one.

mod fixture;
mod journal;
mod safety;

use crate::fixture::{
    finding, mentions, stdout, Fixture, EMPTY_FIELD_ITEM, HEALTHY_ITEM, LOCKED_ITEM, OTHER_SEED,
    STORED_SEED,
};
use crate::journal::{attempt, CODE_ACCEPTED, CODE_REFUSED, FAILED, LOCKED_OUT, SIGNED_IN};

/// An hour is well inside anything, and ten days is the age the six-day loop
/// reached. Both are instants this case writes and then reads back.
const AN_HOUR: i64 = 3600;
const TEN_DAYS: i64 = 864_000;

/// The two halves, joined, over two accounts at once: one whose codes the
/// provider accepted and one refused on every attempt since an instant. The
/// evidence is attributed per account, so one account's history must never
/// decide another's verdict.
#[test]
fn accepted_and_refused_codes_are_told_apart_per_account() {
    let host = Fixture::new();
    host.store_login(LOCKED_ITEM, STORED_SEED);
    host.store_login(HEALTHY_ITEM, OTHER_SEED);
    let refused_since = attempt(LOCKED_ITEM, TEN_DAYS, FAILED, CODE_REFUSED);
    let accepted_at = attempt(HEALTHY_ITEM, AN_HOUR, SIGNED_IN, CODE_ACCEPTED);
    journal::write(
        &host.journal(),
        &[
            attempt(LOCKED_ITEM, TEN_DAYS * 2, SIGNED_IN, CODE_ACCEPTED),
            attempt(HEALTHY_ITEM, TEN_DAYS, SIGNED_IN, CODE_ACCEPTED),
            journal::attempt(LOCKED_ITEM, TEN_DAYS, FAILED, CODE_REFUSED),
            journal::attempt(LOCKED_ITEM, AN_HOUR, FAILED, LOCKED_OUT),
            journal::attempt(HEALTHY_ITEM, AN_HOUR, SIGNED_IN, CODE_ACCEPTED),
        ],
    );

    let (report, output) = host.freshness(None);

    assert!(output.status.success(), "{}", fixture::stderr(&output));
    let locked = finding(&report, LOCKED_ITEM);
    assert_eq!(locked["verdict"], serde_json::json!("seed_rejected_since"));
    assert_eq!(
        locked["rejected_since"],
        serde_json::json!(refused_since.at),
        "the streak starts at the first refusal after the last acceptance, not at the first \
         attempt ever: {locked:#}"
    );
    assert_eq!(locked["locked_out"], serde_json::json!(true));
    assert_eq!(locked["needs_reenrolment"], serde_json::json!(true));

    let healthy = finding(&report, HEALTHY_ITEM);
    assert_eq!(
        healthy["verdict"],
        serde_json::json!("seed_last_known_good")
    );
    assert_eq!(
        healthy["last_known_good_at"],
        serde_json::json!(accepted_at.at),
        "the good row names the instant its code was accepted: {healthy:#}"
    );
    assert_eq!(healthy["repair"], serde_json::Value::Null);
    assert_eq!(healthy["needs_reenrolment"], serde_json::json!(false));
}

/// The repair has to be actionable: the exact command, the real login item,
/// and the fact that Google's lockout blocks re-enrolment until it clears.
#[test]
fn the_repair_names_the_exact_command_and_the_account() {
    let host = Fixture::new();
    host.store_login(LOCKED_ITEM, STORED_SEED);
    journal::write(
        &host.journal(),
        &[
            attempt(LOCKED_ITEM, TEN_DAYS * 2, SIGNED_IN, CODE_ACCEPTED),
            attempt(LOCKED_ITEM, AN_HOUR, FAILED, LOCKED_OUT),
        ],
    );

    let (report, output) = host.freshness(Some(LOCKED_ITEM));

    let repair = finding(&report, LOCKED_ITEM)["repair"]
        .as_str()
        .expect("a row that needs re-enrolment carries its repair")
        .to_string();
    assert!(repair.contains("store-login-totp-seed.sh"), "{repair}");
    assert!(
        repair.contains(&format!("ACCOUNT={LOCKED_ITEM}")),
        "{repair}"
    );
    assert!(repair.contains("re-enrol"), "{repair}");
    assert!(
        repair.contains("locked the authenticator method"),
        "{repair}"
    );
    // The operator reads this on a terminal, not as JSON.
    let lines = stdout(&host.freshness_lines());
    assert!(
        lines.contains("seed_rejected_since") && lines.contains("store-login-totp-seed.sh"),
        "the console hands over the verdict and the repair: {lines}"
    );
    assert!(output.status.success());
}

/// A seed stored after a run of refusals ends the streak. Without this the
/// check would keep telling an operator to re-enrol an account they just
/// repaired.
#[test]
fn an_accepted_code_after_refusals_clears_the_streak() {
    let host = Fixture::new();
    host.store_login(LOCKED_ITEM, STORED_SEED);
    let repaired_at = attempt(LOCKED_ITEM, AN_HOUR, SIGNED_IN, CODE_ACCEPTED);
    journal::write(
        &host.journal(),
        &[
            attempt(LOCKED_ITEM, TEN_DAYS, FAILED, CODE_REFUSED),
            attempt(LOCKED_ITEM, AN_HOUR, SIGNED_IN, CODE_ACCEPTED),
        ],
    );

    let (report, _) = host.freshness(None);

    let row = finding(&report, LOCKED_ITEM);
    assert_eq!(row["verdict"], serde_json::json!("seed_last_known_good"));
    assert_eq!(
        row["last_known_good_at"],
        serde_json::json!(repaired_at.at),
        "the answer is dated by the attempt that repaired it: {row:#}"
    );
    assert_eq!(row["rejected_since"], serde_json::Value::Null);
}

/// A declared field carrying nothing is its own condition, and the vault's
/// answer outranks the run history: a row with no usable seed cannot have had
/// a code accepted, however many acceptances the journal records against it.
#[test]
fn a_declared_field_carrying_nothing_is_not_a_stale_seed() {
    let host = Fixture::new();
    host.store_login(EMPTY_FIELD_ITEM, "");
    journal::write(
        &host.journal(),
        &[attempt(EMPTY_FIELD_ITEM, AN_HOUR, SIGNED_IN, CODE_ACCEPTED)],
    );

    let (report, _) = host.freshness(None);

    let row = finding(&report, EMPTY_FIELD_ITEM);
    assert_eq!(
        row["seed_state"],
        serde_json::json!("declared_empty"),
        "the vault says the field is there and empty: {row:#}"
    );
    assert_eq!(row["verdict"], serde_json::json!("seed_field_empty"));
    assert_eq!(row["needs_reenrolment"], serde_json::json!(true));
    assert!(
        row["repair"]
            .as_str()
            .is_some_and(|repair| repair.contains("store-login-totp-seed.sh")),
        "the repair is to store a seed: {row:#}"
    );
}

/// A seed nothing has ever exercised is untested, not good. Calling it good is
/// how a seed stored years ago and never used reads as healthy.
#[test]
fn a_seed_no_attempt_ever_exercised_is_untested_not_good() {
    let host = Fixture::new();
    host.store_login(LOCKED_ITEM, STORED_SEED);
    host.store_login(HEALTHY_ITEM, OTHER_SEED);
    journal::write(
        &host.journal(),
        &[attempt(HEALTHY_ITEM, AN_HOUR, SIGNED_IN, CODE_ACCEPTED)],
    );

    let (report, _) = host.freshness(None);

    assert_eq!(
        finding(&report, LOCKED_ITEM)["verdict"],
        serde_json::json!("seed_present_untested"),
        "no attempt names this account: {report:#}"
    );
    assert_eq!(
        finding(&report, HEALTHY_ITEM)["verdict"],
        serde_json::json!("seed_last_known_good")
    );
}

/// One account asked about is one account answered about. A fleet-wide sweep
/// of a vault is a different operation from a question about one login.
#[test]
fn asking_about_one_login_item_answers_about_that_one() {
    let host = Fixture::new();
    host.store_login(LOCKED_ITEM, STORED_SEED);
    host.store_login(HEALTHY_ITEM, OTHER_SEED);
    journal::write(
        &host.journal(),
        &[
            attempt(LOCKED_ITEM, TEN_DAYS, FAILED, CODE_REFUSED),
            attempt(HEALTHY_ITEM, AN_HOUR, SIGNED_IN, CODE_ACCEPTED),
        ],
    );

    let (report, _) = host.freshness(Some(LOCKED_ITEM));

    assert_eq!(report["login_rows_read"], serde_json::json!(1));
    assert!(
        !mentions(&report, HEALTHY_ITEM),
        "the account nobody asked about is not in the answer: {report:#}"
    );
    assert_eq!(
        finding(&report, LOCKED_ITEM)["verdict"],
        serde_json::json!("seed_rejected_since")
    );
}
