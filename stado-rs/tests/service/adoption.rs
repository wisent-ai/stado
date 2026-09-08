//! Units the product did not create.
//!
//! Adoption claims what is already on the host, so a fixture that never
//! loaded anything could not test it: the old suite's `adopt` case pointed at
//! a plist path a stand-in `stat` and a stand-in `[ -f ]` rewrite pretended
//! existed, and the "host report" it recorded came from a shell script. Here
//! the unit is a real LaunchAgent this area writes and loads through
//! `/bin/launchctl bootstrap gui/<uid>`, and what the registry ends up
//! holding is checked against what launchd actually has.
//!
//! The distinction between the two removals is the point of the last two
//! cases: `retire` is a management decision and MUST leave the file, and
//! `bootout` addresses a loaded label the registry never declared.

use crate::fixture::fleet::{json_stdout, said, Fleet};
use crate::fixture::unit::{self, Unit};

#[test]
fn adoption_records_the_unit_launchd_is_really_holding() {
    let fleet = Fleet::lifecycle();
    let unit = Unit::loaded(&fleet, &unit::adopted_label("adopt"));
    let pid = unit.await_pid();

    let out = fleet.stado(&[
        "service",
        "adopt",
        &unit.label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(out.status.success(), "adopt failed: {}", said(&out));
    let report = json_stdout(&out);
    assert_eq!(report["action"], "adopted");
    assert_eq!(report["service"]["host"], fleet.target);
    assert_eq!(report["service"]["label"], unit.label);
    assert_eq!(report["service"]["kind"], "launchd");
    // The path is the host's answer, not the caller's: adoption probed the
    // machine and found the file launchd loaded.
    assert_eq!(report["service"]["path"], unit.plist_arg());
    assert_eq!(report["remote"]["status"], "probed");
    assert_eq!(report["remote"]["path"], unit.plist_arg());
    assert_eq!(report["remote"]["launchd_domain"]["name"], unit.domain);

    let record = crate::only_record(&fleet);
    assert_eq!(record["label"], unit.label);
    assert_eq!(record["kind"], "launchd");
    assert_eq!(record["path"], unit.plist_arg());

    // Adoption claims; it does not touch. The same job is still running.
    assert_eq!(
        unit.live_pid(),
        Some(pid),
        "adoption disturbed the job it was only supposed to claim"
    );
}

#[test]
fn retirement_stops_the_unit_and_deliberately_leaves_its_file_on_disk() {
    let fleet = Fleet::lifecycle();
    let unit = Unit::loaded(&fleet, &unit::adopted_label("retire"));
    unit.await_pid();
    let adopted = fleet.stado(&[
        "service",
        "adopt",
        &unit.label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(adopted.status.success(), "adopt failed: {}", said(&adopted));

    let out = fleet.stado(&[
        "service",
        "retire",
        &unit.label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(out.status.success(), "retire failed: {}", said(&out));
    let report = json_stdout(&out);
    assert_eq!(report["action"], "retired");
    assert_eq!(report["remote"]["status"], "retired");
    assert_eq!(report["remote"]["postcondition"]["state"], "met");
    assert_eq!(
        report["remote"]["postcondition"]["detail"],
        format!("no job at {}", unit.qualified())
    );

    assert!(
        unit.absence().contains(&format!(
            "Could not find service \"{}\" in domain for user gui",
            unit.label
        )),
        "launchd still holds the retired label: {}",
        unit.absence()
    );
    assert!(
        fleet.declared().is_empty(),
        "the retired service is still declared: {:#?}",
        fleet.declared()
    );
    // The one thing retirement must NOT do.
    assert!(
        unit.plist.exists(),
        "retirement deleted {}; that is what `remove` is for",
        unit.plist_arg()
    );
}

#[test]
fn bootout_stops_a_loaded_label_the_registry_never_declared() {
    let fleet = Fleet::lifecycle();
    let unit = Unit::loaded(&fleet, &unit::adopted_label("bootout"));
    unit.await_pid();
    // Nothing declares it: `stop` and `retire` would refuse, and this verb
    // exists for exactly that state.
    assert!(
        fleet.declared().is_empty(),
        "the fixture declared a service"
    );

    let out = fleet.stado(&[
        "service",
        "bootout",
        &unit.label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(out.status.success(), "bootout failed: {}", said(&out));
    let report = json_stdout(&out);
    assert_eq!(report["host"], fleet.target);
    assert_eq!(report["label"], unit.label);
    assert_eq!(report["state"], "booted_out");
    assert_eq!(report["detail"], unit.qualified());

    assert!(
        unit.absence().contains(&format!(
            "Could not find service \"{}\" in domain for user gui",
            unit.label
        )),
        "launchd still holds the label: {}",
        unit.absence()
    );
    assert!(unit.live_pid().is_none(), "launchd still holds a pid");
    // It never removes the unit's file, and it never invents a declaration.
    assert!(unit.plist.exists(), "bootout deleted the unit file");
    assert!(
        fleet.declared().is_empty(),
        "bootout declared something: {:#?}",
        fleet.declared()
    );
}
