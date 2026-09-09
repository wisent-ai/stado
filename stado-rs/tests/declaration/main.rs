//! A unit declaration against the launchd domain its host can actually have.
//!
//! `com.wisent.compute.service.stado-agent-mini` was declared as a user
//! LaunchAgent at `/Users/charles/Library/LaunchAgents/...` on an always-on
//! host. That host is declared always-on in both `role` and `host_heuristic`
//! and has no graphical session at all: `/dev/console` is root's, `who` prints
//! nothing, `loginwindow` runs as root, and the login's own `launchctl list`
//! holds no `com.wisent.*` label. So `launchctl bootstrap user/501 <plist>`
//! answers `Bootstrap failed: 5: Input/output error` there, `gui/501` does not
//! exist, and the declaration named a domain that could never load the unit.
//! Every other always-on unit on that host is a system LaunchDaemon under
//! `/Library/LaunchDaemons`.
//!
//! Nothing about the headless case needs a host: the path says the domain and
//! the target says both that it runs unattended and that it owns no graphical
//! workload. `always-on` alone is not enough — the Mac mini is always-on and
//! keeps an autologin Aqua session for Weles. These tests defend both cases:
//! a truly headless host reports the domain mismatch, while an always-on Weles
//! host keeps its LaunchAgents valid.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! into a test, and HOME is inside the temp dir. No host is contacted: both
//! commands under test answer from the registry document and the beacon
//! objects in that storage root alone, which is why the two host names here
//! are declaration rows and not machines.
//!
//! Every sentence asserted here was copied from a hand run against this
//! seeded state, never guessed. Repaired on 2026-09-08: the fixture declared
//! `weles: null` on a non-graphical host, which the schema refuses, so the
//! corrected-declaration case was failing on `main`; the key is omitted now.

mod cases;
mod fixture;

use fixture::{
    domain_findings, sentence, service_row, stdout, Harness, UnitSpec, ACCOUNT, AGENT, AGENT_PATH,
    ALWAYS_ON_HOST, INSTALL_COMMAND, INTERACTIVE_HOST, STREAM, STREAM_PATH, WELES, WELES_PATH,
};

/// A user agent declared on an always-on host is a `registry doctor` finding,
/// in one sentence naming the unit, both domains, and the privileged command.
#[test]
fn doctor_reports_a_user_agent_declared_on_an_always_on_host() {
    let harness = Harness::new();
    harness.declare_registry(
        false,
        &UnitSpec::new(AGENT, AGENT_PATH),
        &UnitSpec::new(STREAM, STREAM_PATH),
    );
    harness.declare_beacon(ALWAYS_ON_HOST, &[WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    let out = harness.stado(&["registry", "doctor", "--json"]);
    let findings = domain_findings(&out);

    assert_eq!(findings.len(), 1, "one row, for the one misdeclared unit");
    assert_eq!(
        findings[0]
            .get("subject")
            .and_then(serde_json::Value::as_str),
        Some(ALWAYS_ON_HOST)
    );
    assert_eq!(
        findings[0]
            .get("detail")
            .and_then(serde_json::Value::as_str),
        Some(sentence().as_str())
    );
    // The sentence has to carry the command an operator runs next, verbatim,
    // and that command has to keep the job on the account that owns the agent:
    // a daemon without `UserName` runs the fleet's binary as uid 0 against an
    // account-owned `~/.stado`.
    let detail = findings[0]
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .expect("a detail sentence");
    assert!(
        detail.contains(INSTALL_COMMAND),
        "the finding names the privileged install command"
    );
    assert!(
        detail.contains(&format!(
            "/usr/bin/plutil -insert UserName -string {ACCOUNT} "
        )),
        "the install command keeps the daemon running as {ACCOUNT}"
    );
    // A divergence exits non-zero, the way every other doctor finding does.
    assert!(!out.status.success(), "doctor fails on a divergence");
}

/// The same declaration on an interactive host is correct, and the check says
/// nothing at all about it — not about the unit, and not about the host.
#[test]
fn doctor_stays_silent_for_a_user_agent_on_an_interactive_host() {
    let harness = Harness::new();
    // Both hosts declare a per-account LaunchAgent; only the always-on one is
    // a finding, so the interactive row is the control.
    harness.declare_registry(
        false,
        &UnitSpec::new(WELES, WELES_PATH),
        &UnitSpec::new(STREAM, STREAM_PATH),
    );
    harness.declare_beacon(ALWAYS_ON_HOST, &[WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    let out = harness.stado(&["registry", "doctor", "--json"]);

    assert!(
        domain_findings(&out).is_empty(),
        "a user agent on an interactive host is the right declaration"
    );
    assert!(
        !stdout(&out).contains("misdeclared-domain"),
        "nothing in the report mentions the finding"
    );
}

/// Uptime and a graphical login are independent. An always-on target that
/// owns Weles keeps an Aqua account alive, so its LaunchAgents are deliberate.
#[test]
fn doctor_keeps_user_agents_on_an_always_on_weles_host() {
    let harness = Harness::new();
    harness.declare_registry(
        true,
        &UnitSpec::new(AGENT, AGENT_PATH),
        &UnitSpec::new(STREAM, STREAM_PATH),
    );
    harness.declare_beacon(ALWAYS_ON_HOST, &[AGENT, WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    let doctor = harness.stado(&["registry", "doctor", "--json"]);
    assert!(
        domain_findings(&doctor).is_empty(),
        "always-on does not mean headless when the target declares Weles"
    );

    let services = harness.stado(&["service", "list", "--json"]);
    assert!(
        service_row(&services, AGENT)
            .get("misdeclared_domain")
            .is_none(),
        "the graphical host's user service is valid"
    );
}
