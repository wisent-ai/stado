//! The refusals of the lifecycle, on this machine, with nothing loaded.
//!
//! Every sentence below is copied from a live run on this host, and every
//! case ends by asking launchd whether anything was installed anyway. The
//! privileged case is the one the old suite faked hardest: it asserted the
//! system-daemon path out of a pure function
//! (`ensure_unit_path(&daemon) == "/Library/LaunchDaemons/<label>.plist"`)
//! against an invented target, which is true whether or not the command can
//! do anything with it. This login has no passwordless sudo, so the honest
//! evidence is the refusal — it names the exact privileged step that was
//! refused, and the system domain is then read to prove nothing landed.

use crate::fixture::fleet::{said, Fleet};
use crate::fixture::unit::{self, Unit, HOLD_SECONDS, PROGRAM};

#[test]
fn the_privileged_system_domain_is_refused_by_name_and_installs_nothing() {
    let fleet = Fleet::lifecycle();
    let name = unit::service_name("daemon");
    let unit = Unit::claim(&fleet, &name);
    let daemon_file = format!("/Library/LaunchDaemons/{}.plist", unit.label);
    assert!(
        !std::path::Path::new(&daemon_file).exists(),
        "{daemon_file} already exists; this case would be reading someone else's unit"
    );

    let out = fleet.stado(&[
        "service",
        "ensure",
        &name,
        "--host",
        &fleet.target,
        "--from",
        PROGRAM,
        "--arg",
        HOLD_SECONDS,
        "--as-daemon",
        "--reason",
        "prove the privileged domain is refused rather than faked",
        "--json",
    ]);
    assert!(!out.status.success(), "ensure --as-daemon was not refused");
    let told = said(&out);
    // The step, spelled out: an operator reading this knows which grant is
    // missing without going to look.
    assert!(
        told.contains(&format!("sudo -n install {daemon_file} was refused")),
        "the refusal did not name the privileged install: {told}"
    );
    assert!(
        told.contains(&format!(
            "postcondition unmet: the unit is loaded and has a pid (no job at system/{})",
            unit.label
        )),
        "the refusal did not report the system domain as empty: {told}"
    );

    // Nothing in the machine's own domain, and nothing recorded.
    assert!(
        !std::path::Path::new(&daemon_file).exists(),
        "a refused daemon install left {daemon_file} behind"
    );
    let system = unit::launchctl(&["print", &format!("system/{}", unit.label)]);
    assert!(
        !system.status.success(),
        "launchd's system domain holds {}: {}",
        unit.label,
        said(&system)
    );
    assert!(
        fleet.declared().is_empty(),
        "a refused ensure recorded a service: {:#?}",
        fleet.declared()
    );
}

#[test]
fn a_program_the_host_does_not_have_is_refused_and_no_unit_is_loaded() {
    let fleet = Fleet::lifecycle();
    let name = unit::service_name("absent");
    let unit = Unit::claim(&fleet, &name);
    let absent = fleet.root().join("no-such-program");

    let out = fleet.stado(&[
        "service",
        "ensure",
        &name,
        "--host",
        &fleet.target,
        "--from",
        absent.to_str().expect("a UTF-8 fixture path"),
        "--reason",
        "a program this machine does not have",
        "--json",
    ]);
    assert!(
        !out.status.success(),
        "ensure accepted a program that is not there"
    );
    let told = said(&out);
    assert!(
        told.contains(&format!("program_missing: {}", absent.display())),
        "the refusal did not name the missing program: {told}"
    );

    assert!(!unit.plist.exists(), "a refused ensure wrote a unit file");
    assert!(unit.printed().is_none(), "a refused ensure loaded a job");
    assert!(
        fleet.declared().is_empty(),
        "a refused ensure recorded a service: {:#?}",
        fleet.declared()
    );
}

#[test]
fn a_change_with_no_recorded_reason_is_refused_before_the_host_is_touched() {
    let fleet = Fleet::lifecycle();
    let name = unit::service_name("reason");
    let unit = Unit::claim(&fleet, &name);

    let out = fleet.stado(&[
        "service",
        "ensure",
        &name,
        "--host",
        &fleet.target,
        "--from",
        PROGRAM,
        "--arg",
        HOLD_SECONDS,
        "--reason",
        "   ",
        "--json",
    ]);
    assert!(!out.status.success(), "ensure accepted a blank reason");
    assert!(
        said(&out).contains(
            "--reason must say why this host has to run this unit; it is recorded beside the \
             registry document this command declares the unit in"
        ),
        "got: {}",
        said(&out)
    );

    assert!(!unit.plist.exists(), "a refused ensure wrote a unit file");
    assert!(unit.printed().is_none(), "a refused ensure loaded a job");
}
