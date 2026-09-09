//! The two repairs `service restart` and `service stop` perform on a unit
//! this login owns, driven against this machine's own launchd.
//!
//! Both cases are the shape of the outage this area exists for: a managed
//! unit the registry declares, whose job launchd is or is not holding, and a
//! command that has to leave the host in a state an operator can read. So
//! neither case trusts the report — it asks launchd afterwards, and the
//! report is corroboration.

use crate::fixture::fleet::{json_stdout, launchd_record, said, Fleet};
use crate::fixture::unit::{gui_domain, Unit, PROGRAM};

/// A launchd label unique to this test process and this case, under the
/// prefix the product's own unit enumeration accepts, matching nothing in the
/// operator's fleet.
fn label(case: &str) -> String {
    format!(
        "{}stado-test.recovery-{}-{case}",
        stado::deploy::local_install::FLEET_LABEL_PREFIX,
        std::process::id()
    )
}

/// A fleet of this machine declaring one per-login unit whose file is on
/// disk, and the guard that boots its label out when the case ends.
fn declared(case: &str) -> (Fleet, Unit) {
    let fleet = Fleet::new();
    let unit = Unit::claim(&fleet, &label(case));
    unit.write_plist();
    fleet.declare(launchd_record(&unit.label, &unit.plist_arg()));
    (fleet, unit)
}

/// The unloaded half of the 2026-08-19 shape: the unit file is on the host,
/// the registry declares it, and launchd holds no job under the label. A
/// restart has to load it into the domain this login actually has, and the
/// pid it reports has to be the pid launchd is holding.
#[test]
fn restart_loads_a_declared_unit_launchd_is_holding_no_job_for() {
    let (fleet, unit) = declared("reload");
    assert!(
        unit.printed().is_none(),
        "launchd already holds {}",
        unit.qualified()
    );

    let out = fleet.stado(&[
        "service",
        "restart",
        &unit.label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(out.status.success(), "restart failed: {}", said(&out));

    // The state the pass left: launchd is holding a job under the label, from
    // the file the registry declared, and it has a pid.
    let pid = unit.await_pid();
    let printed = unit
        .printed()
        .unwrap_or_else(|| panic!("launchd holds no job at {}", unit.qualified()));
    assert!(
        printed.contains(&format!("path = {}", unit.printed_path())),
        "launchd loaded some other file: {printed}"
    );
    assert!(
        printed.contains(&format!("program = {PROGRAM}")),
        "launchd is not running the declared program: {printed}"
    );

    let report = &json_stdout(&out)[0];
    assert_eq!(report["host"], fleet.target);
    assert_eq!(report["os"], "Darwin");
    assert_eq!(report["status"], "restarted");
    assert_eq!(report["path"], unit.plist_arg());
    // The domain is the whole point: a LaunchAgent of this login belongs to
    // `gui/<uid>`, and a restart aimed anywhere else silently does nothing.
    assert_eq!(report["launchd_domain"]["name"], gui_domain());
    assert_eq!(report["launchd_domain"]["status"], "graphical");
    assert_eq!(report["detail"], gui_domain());
    assert_eq!(
        report["postcondition"]["intent"],
        "the unit is loaded and has a pid"
    );
    assert_eq!(report["postcondition"]["state"], "met");
    assert_eq!(
        report["postcondition"]["detail"],
        format!("{} pid {pid}", unit.qualified()),
        "the postcondition names a pid launchd does not hold"
    );
}

/// `service stop` is the fenced half of a recovery cutover: the job goes
/// away, and the unit file and its declaration deliberately stay, so the
/// same unit can be brought back. Both halves are read off the host.
#[test]
fn stop_boots_the_unit_out_of_this_logins_domain_and_keeps_its_declaration() {
    let (fleet, unit) = declared("stop");
    let running = unit.load();
    let declared_before = fleet.registry_bytes();

    let out = fleet.stado(&[
        "service",
        "stop",
        &unit.label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(out.status.success(), "stop failed: {}", said(&out));

    // launchd's own words about the label, waited for rather than assumed.
    let absence = unit.absence();
    assert!(
        absence.contains(&format!(
            "Could not find service \"{}\" in domain for user gui: {}",
            unit.label,
            crate::fixture::unit::uid()
        )),
        "launchd still holds the job: {absence}"
    );
    assert!(
        unit.live_pid().is_none(),
        "launchd still reports a pid for {}, which was {running} before the stop",
        unit.label
    );
    // What a stop must NOT do, because a cutover has to be reversible.
    assert!(
        unit.plist.exists(),
        "a stop deleted the unit file at {}",
        unit.plist_arg()
    );
    assert_eq!(
        fleet.registry_bytes(),
        declared_before,
        "a stop withdrew the declaration"
    );

    let report = &json_stdout(&out)[0];
    assert_eq!(report["host"], fleet.target);
    assert_eq!(report["status"], "stopped");
    assert_eq!(report["launchd_domain"]["name"], gui_domain());
    assert_eq!(report["postcondition"]["intent"], "the unit is not running");
    assert_eq!(report["postcondition"]["state"], "met");
    assert_eq!(
        report["postcondition"]["detail"],
        format!("no job at {}", unit.qualified())
    );
}
