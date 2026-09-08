//! The service lifecycle, run against this machine's own launchd.
//!
//! What this area used to be: a script named `ssh` on PATH that piped the
//! product's remote program into a local `bash`, a directory of stand-in
//! `launchctl`, `systemctl`, `journalctl`, `plutil`, `sudo`, `stat`, `id`,
//! `uname` and `sleep` executables, an invented host (`w1`, `linux-builder`)
//! with an invented destination (`u@10.0.0.1`, `approved@10.9.9.20`), and
//! assertions that read a call log those stand-ins wrote. No unit was ever
//! installed, loaded, restarted or removed, so nothing in it was evidence
//! that Stado can put a service on a host and take it off again.
//!
//! What it is now: one registry target naming THIS machine, no substituted
//! executable anywhere on PATH, and the product's own current-host path
//! installing, converging, reading and removing a real LaunchAgent in this
//! login's own `gui/<uid>` domain. Every label is unique to this test process
//! (`fixture::unit::adopted_label`, `product_label`), so no unit the operator
//! runs is addressable from here, and [`fixture::unit::Unit`]'s `Drop` boots
//! the label out even when a case panics. Assertions read state: the plist on
//! disk, `launchctl print`, `launchctl list`, the persisted registry
//! document, the exit code. Stdout corroborates; it is never the only
//! witness.
//!
//! Where each promise is proved:
//!
//! * a declared unit is installed, loaded and recorded — this file;
//! * a second pass converges the same label instead of adding another — this
//!   file;
//! * removal boots it out, deletes the file and withdraws the declaration,
//!   and launchd is asked to confirm it — this file;
//! * restart, log tail and environment readback — `launchd.rs`;
//! * a unit the product did not create: adopt, retire, bootout —
//!   `adoption.rs`;
//! * the declaration contract and its refusals — `declaration.rs`;
//! * the refusals of the lifecycle itself, including the privilege this test
//!   does not have — `refusals.rs`.
//!
//! Not covered here, and deliberately not faked: every Linux/systemd case the
//! old suite claimed (they need a second, non-Darwin registered host), and
//! the privileged `/Library/LaunchDaemons` half of `ensure` (it needs
//! passwordless sudo, which this login does not have — `refusals.rs` proves
//! the refusal instead).

mod adoption;
mod declaration;
mod fixture;
mod launchd;
mod refusals;

use serde_json::Value;

use fixture::fleet::{json_stdout, said, Fleet};
use fixture::unit::{Unit, HOLD_SECONDS, PROGRAM};

/// `service ensure NAME --host <this machine> --from /bin/sleep`: the one
/// command that installs a unit, and the only writer of the label the guard
/// claimed.
pub fn ensure(fleet: &Fleet, name: &str, reason: &str) -> std::process::Output {
    fleet.stado(&[
        "service",
        "ensure",
        name,
        "--host",
        &fleet.target,
        "--from",
        PROGRAM,
        "--arg",
        HOLD_SECONDS,
        "--reason",
        reason,
        "--json",
    ])
}

/// The one `services[]` record this machine's target declares.
pub fn only_record(fleet: &Fleet) -> Value {
    let declared = fleet.declared();
    assert_eq!(
        declared.len(),
        1,
        "expected exactly one declared service, got {declared:#?}"
    );
    declared.into_iter().next().expect("one record")
}

#[test]
fn an_ensured_unit_is_installed_into_launchd_and_recorded_in_the_registry() {
    let fleet = Fleet::lifecycle();
    let name = fixture::unit::service_name("install");
    let unit = Unit::claim(&fleet, &name);

    let out = ensure(
        &fleet,
        &name,
        "prove the declared unit reaches this machine's launchd",
    );
    assert!(out.status.success(), "ensure failed: {}", said(&out));
    let report = json_stdout(&out);
    assert_eq!(report["host"], fleet.target);
    assert_eq!(report["label"], unit.label);
    assert_eq!(report["action"], "created");
    // `domain_word` is two-valued on purpose: a per-login job, not a daemon.
    assert_eq!(report["domain"], "user");

    // The file the product wrote, read off disk.
    let plist = std::fs::read_to_string(&unit.plist).expect("the product installed no plist");
    assert!(
        plist.contains(&unit.label),
        "the plist names another label:\n{plist}"
    );
    assert!(
        plist.contains(PROGRAM),
        "the plist names another program:\n{plist}"
    );

    // launchd's own answer about that file.
    let printed = unit
        .printed()
        .expect("launchd holds no job under the label");
    assert!(
        printed.contains(&format!("path = {}", unit.printed_path())),
        "launchd loaded another file:\n{printed}"
    );
    assert!(
        printed.contains(&format!("program = {PROGRAM}")),
        "launchd runs another program:\n{printed}"
    );
    assert_eq!(
        report["pid"].as_u64().map(|pid| pid as u32),
        unit.live_pid(),
        "the reported pid is not the one launchd holds"
    );

    // The declaration the pass persisted.
    let record = only_record(&fleet);
    assert_eq!(record["label"], unit.label);
    assert_eq!(record["kind"], "launchd");
    assert_eq!(record["path"], unit.plist_arg());
    assert_eq!(record["program"], PROGRAM);
    assert_eq!(record["args"][0], HOLD_SECONDS);
}

#[test]
fn a_second_pass_converges_the_same_label_instead_of_installing_a_second_unit() {
    let fleet = Fleet::lifecycle();
    let name = fixture::unit::service_name("converge");
    let unit = Unit::claim(&fleet, &name);

    let first = ensure(&fleet, &name, "install the unit this pass will converge");
    assert!(
        first.status.success(),
        "first ensure failed: {}",
        said(&first)
    );

    let again = ensure(&fleet, &name, "assert the same unit a second time");
    assert!(
        again.status.success(),
        "second ensure failed: {}",
        said(&again)
    );
    let report = json_stdout(&again);
    assert_eq!(report["label"], unit.label);
    assert_eq!(report["action"], "converged");
    assert_eq!(
        report["pid"].as_u64().map(|pid| pid as u32),
        unit.live_pid(),
        "the converged pass did not report the pid launchd holds"
    );

    // One label, one job, one record: this is what "idempotent" has to mean.
    let listed = fixture::unit::launchctl(&["list"]);
    let holding = said(&listed)
        .lines()
        .filter(|line| line.ends_with(&unit.label))
        .count();
    assert_eq!(
        holding, 1,
        "launchd holds {holding} jobs under {}",
        unit.label
    );
    let record = only_record(&fleet);
    assert_eq!(record["label"], unit.label);
}

#[test]
fn removal_boots_the_unit_out_deletes_its_file_and_withdraws_the_declaration() {
    let fleet = Fleet::lifecycle();
    let name = fixture::unit::service_name("remove");
    let unit = Unit::claim(&fleet, &name);
    let installed = ensure(&fleet, &name, "install the unit this case removes");
    assert!(
        installed.status.success(),
        "ensure failed: {}",
        said(&installed)
    );
    unit.await_pid();

    let out = fleet.stado(&[
        "service",
        "remove",
        &unit.label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(out.status.success(), "remove failed: {}", said(&out));
    let report = json_stdout(&out);
    assert_eq!(report["action"], "removed");
    assert_eq!(report["report"]["postcondition"]["state"], "met");
    assert_eq!(
        report["report"]["postcondition"]["detail"],
        format!("no job at {}", unit.qualified())
    );
    assert_eq!(report["file"]["status"], "removed");
    assert_eq!(report["file"]["path"], unit.plist_arg());

    // The three places the removal has to be true, read rather than trusted.
    assert!(!unit.plist.exists(), "the unit file survived removal");
    assert!(
        unit.absence().contains(&format!(
            "Could not find service \"{}\" in domain for user gui",
            unit.label
        )),
        "launchd did not answer that the label is gone: {}",
        unit.absence()
    );
    assert!(unit.live_pid().is_none(), "launchd still holds a pid");
    assert!(
        fleet.declared().is_empty(),
        "the declaration survived removal: {:#?}",
        fleet.declared()
    );
}
