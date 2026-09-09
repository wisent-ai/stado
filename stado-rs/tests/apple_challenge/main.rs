//! Apple preparation on a registered Darwin ARM64 host, and the boundary in
//! front of it.
//!
//! The preparation itself issues an Apple-challenge credential, which means an
//! Account Holder standing at an Apple sign-in prompt on that machine. That is
//! one case, and it is gated: its reason names the host variable it needs, the
//! consent it needs, and the exact command that runs it.
//!
//! Everything in front of that consent runs by default, because none of it
//! reaches a machine. [`refusals`] holds those cases: which host the fleet will
//! place the preparation on, which plan it accepts, and the complete report it
//! publishes for a declared Darwin ARM64 host it cannot reach — including the
//! Apple-only preparation itself, stopped at the brokered key with this
//! machine's own GUI state untouched.
//!
//! A predecessor of this area drove `stado host gui-automation status` and
//! `stado host gui-automation grant-accessibility`. Neither subcommand exists
//! in this product — the surface is `stado workload run|status gui-automation`
//! — so both cases could only ever have failed with `unrecognized subcommand`,
//! and behind their ignore nobody found out. Every command below was run
//! against the built binary.
//!
//! The files are split so each stays inside the three hundred line limit this
//! repository enforces on itself.

mod fixture;
mod refusals;

use serde_json::json;

use crate::fixture::{report, said, PLAN_SCHEMA};

/// The registered host the preparation is performed on. There is no default: a
/// case that chose an Apple host itself would put a consent prompt in front of
/// whoever happens to be sitting at it.
const HOST_VARIABLE: &str = "STADO_APPLE_PREPARATION_HOST";

/// The built binary with the operator's own registry and configuration, which
/// is what a real host journey needs: the host key is brokered through them.
fn stado(arguments: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .env("NO_COLOR", "1")
        .output()
        .expect("the built Stado binary starts")
}

/// The GUI state a host publishes, as a sorted list of pairs.
fn state(document: &serde_json::Value) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = document["items"]
        .as_array()
        .expect("a reachable host publishes its state items")
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().unwrap_or_default().to_string(),
                pair[1].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    items.sort();
    items
}

/// The state that is not the Apple challenge's own, which an Apple-only
/// preparation must leave exactly as it found it.
fn unrelated(document: &serde_json::Value) -> Vec<(String, String)> {
    state(document)
        .into_iter()
        .filter(|(key, _)| !key.starts_with("apple-challenge-"))
        .collect()
}

/// The real Apple-only preparation: it issues the Apple-challenge credential on
/// the registered host, so it cannot run without an Account Holder answering
/// the Apple sign-in prompt on that machine.
///
/// It asserts the state the host publishes afterwards — the helper is present
/// at its declared version, accessibility is granted, the challenge reports
/// itself ready, and every GUI fact that is not the Apple challenge's own is
/// byte-identical to what the host published before.
#[test]
#[ignore = "issues the Apple challenge credential on a registered Darwin ARM64 host: needs \
            STADO_APPLE_PREPARATION_HOST naming that host in the canonical registry, its readable \
            host-account credential, and an Account Holder answering the Apple sign-in prompt on \
            that machine. Run it with: STADO_APPLE_PREPARATION_HOST=<host> cargo test --test \
            apple_challenge -- --ignored \
            apple_only_preparation_issues_the_credential_and_preserves_other_gui_state"]
fn apple_only_preparation_issues_the_credential_and_preserves_other_gui_state() {
    assert_eq!(std::env::consts::OS, "macos");
    assert_eq!(std::env::consts::ARCH, "aarch64");
    let host = std::env::var(HOST_VARIABLE)
        .unwrap_or_else(|_| panic!("{HOST_VARIABLE} must name the registered Apple host"));
    assert!(!host.trim().is_empty(), "the Apple host must be explicit");

    let observed = stado(&[
        "workload",
        "status",
        "gui-automation",
        "--target",
        &host,
        "--json",
    ]);
    assert!(observed.status.success(), "{}", said(&observed));
    let before = report(&observed);

    let work = tempfile::tempdir().expect("a directory this case owns exists");
    let plan = work.path().join("apple-only-preparation-plan.json");
    std::fs::write(
        &plan,
        serde_json::to_vec_pretty(&json!({
            "schema": PLAN_SCHEMA,
            "operation": "grant-accessibility",
            "apple_only": true,
        }))
        .expect("serialize the preparation plan"),
    )
    .expect("write the preparation plan");
    let prepared = stado(&[
        "workload",
        "run",
        "gui-automation",
        "--target",
        &host,
        "--plan",
        plan.to_str().expect("a UTF-8 plan path"),
        "--json",
    ]);
    assert!(prepared.status.success(), "{}", said(&prepared));
    assert_eq!(report(&prepared)["error"], serde_json::Value::Null);

    let observed = stado(&[
        "workload",
        "status",
        "gui-automation",
        "--target",
        &host,
        "--json",
    ]);
    assert!(observed.status.success(), "{}", said(&observed));
    let after = report(&observed);
    let published = state(&after);
    let value = |key: &str| {
        published
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };

    assert_eq!(after["target"], host.as_str());
    assert_eq!(after["error"], serde_json::Value::Null, "{after}");
    assert_eq!(value("apple-challenge-accessibility").as_deref(), Some("granted"));
    assert_eq!(value("apple-challenge-ready").as_deref(), Some("yes"), "{after}");
    assert_eq!(
        value("console"),
        value("accessibility-user"),
        "the granted user is not the one at the console: {after}",
    );
    assert_eq!(after["ssh_target"], before["ssh_target"]);
    assert_eq!(
        unrelated(&after),
        unrelated(&before),
        "Apple-only preparation changed unrelated GUI state",
    );
}
