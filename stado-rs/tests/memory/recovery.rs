//! The declared recovery program's own precondition, reached through the
//! janitor pass.
//!
//! On 2026-09-22 charless-mac-mini's memory policy declared
//! `reap_recovery: recover-skarbiec-crypto`, `keyboxd` held 15 GiB after
//! twelve days, and every pass ended with the program refusing: Skarbiec's
//! readiness answered ok, and memory was not one of the program's reasons.
//! The refusal said nothing about memory, so the pass read as a healthy host
//! with nothing to reap. The program now measures the account's `keyboxd` and
//! `gpg-agent` against `SKARBIEC_GPG_DAEMON_MEMORY_LIMIT_MB` and refuses only
//! when neither is over it, saying so.
//!
//! This runs the real program on the real host through the real pass. The
//! readiness endpoint it probes is a loopback port the kernel just handed
//! back and nothing serves, and the ceiling it applies is one no daemon
//! reaches, so the one outcome it can have here is the refusal — and that
//! refusal must now carry the ceiling it measured against. The operator's own
//! daemons are never signalled: the program stops at its precondition.

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

use super::*;
use crate::constants::{SKARBIEC_CRYPTO_RECOVERY, UNREACHABLE_DAEMON_CEILING_MB};
use crate::harness::run_pass_with_env;

/// A loopback URL nothing answers on: the kernel reserves a free port and it
/// is released before the pass runs.
fn unserved_readiness_url() -> String {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("reserve a loopback port nothing serves");
    let port = listener
        .local_addr()
        .expect("the reserved port has an address")
        .port();
    drop(listener);
    format!("http://127.0.0.1:{port}/readyz")
}

#[test]
fn a_declared_recovery_that_finds_no_wedge_and_no_bloated_daemon_names_the_ceiling_it_refused_on() {
    let storage = setup();
    let declared = declare(
        storage.path(),
        &[
            "--memory-mode",
            "enforce",
            "--memory-repair",
            "reap_recovery",
            "--memory-repair-recovery",
            SKARBIEC_CRYPTO_RECOVERY,
        ],
    );
    assert!(
        declared.status.success(),
        "declaring the recovery failed: {}",
        stderr(&declared)
    );

    let readiness = unserved_readiness_url();
    let report = run_pass_with_env(
        storage.path(),
        &[
            ("SKARBIEC_READY_URL", readiness.as_str()),
            (
                "SKARBIEC_GPG_DAEMON_MEMORY_LIMIT_MB",
                UNREACHABLE_DAEMON_CEILING_MB,
            ),
        ],
    );
    assert_eq!(report["mode"], serde_json::json!("enforce"));
    let repair = &report["repairs"]["reap_recovery"];
    assert_eq!(
        repair["subjects"],
        serde_json::json!([SKARBIEC_CRYPTO_RECOVERY]),
        "the pass did not record the declared program: {report}"
    );
    assert_eq!(
        repair["skipped"]["recovery_refused"],
        serde_json::json!(1),
        "the pass did not record the program's refusal: {report}"
    );
    assert_eq!(repair["repaired"], serde_json::json!(0));
    let refusal = report["errors"]
        .as_array()
        .and_then(|errors| errors.iter().find_map(serde_json::Value::as_str))
        .unwrap_or_else(|| panic!("the pass recorded no refusal sentence: {report}"));
    assert!(
        refusal.contains(&format!(
            "no GnuPG daemon of this account stands over the {UNREACHABLE_DAEMON_CEILING_MB} MiB ceiling"
        )),
        "the refusal does not name the ceiling the program measured against: {refusal}"
    );
    assert!(
        refusal.contains("did not report a GPG failure or keybox lock"),
        "the refusal does not name the readiness reason it also lacked: {refusal}"
    );
}
