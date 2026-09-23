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

/// A real executable under the file name the shipped catalog gives
/// Skarbiec's one process: a copy of this machine's `/bin/sleep`, so the unit
/// it would start stays up exactly like every other unit in this area.
fn product_executable(fleet: &Fleet) -> String {
    use std::os::unix::fs::PermissionsExt;
    let directory = fleet.root().join("bin");
    std::fs::create_dir_all(&directory).expect("a directory for the product executable");
    let path = directory.join("skarbiec");
    std::fs::copy(PROGRAM, &path).expect("copy /bin/sleep under the product's file name");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make the product executable runnable");
    path.display().to_string()
}

fn ensure_from(fleet: &Fleet, name: &str, program: &str, reason: &str) -> std::process::Output {
    fleet.stado(&[
        "service",
        "ensure",
        name,
        "--host",
        &fleet.target,
        "--from",
        program,
        "--arg",
        HOLD_SECONDS,
        "--reason",
        reason,
        "--json",
    ])
}

#[test]
fn a_new_unit_running_a_products_executable_is_refused_as_a_second_process() {
    let fleet = Fleet::lifecycle();
    let name = unit::service_name("second-process");
    let unit = Unit::claim(&fleet, &name);
    let program = product_executable(&fleet);

    let out = ensure_from(&fleet, &name, &program, "a helper beside the vault");
    assert!(
        !out.status.success(),
        "ensure started a second skarbiec process"
    );
    let told = said(&out);
    assert!(
        told.contains(&format!(
            "{} would run {program}, a second skarbiec process beside its one unit \
             com.wisent.always-on.skarbiec; a product runs as one process per host, so move \
             this work into skarbiec and deploy skarbiec instead",
            unit.label
        )),
        "the refusal did not name the product and its one unit: {told}"
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
fn a_declared_unit_may_not_switch_to_a_products_executable_but_one_that_already_ran_it_is_repaired(
) {
    let fleet = Fleet::lifecycle();
    let name = unit::service_name("switch");
    let unit = Unit::claim(&fleet, &name);
    let program = product_executable(&fleet);

    let first = ensure_from(&fleet, &name, PROGRAM, "a unit that is no product");
    assert!(first.status.success(), "ensure failed: {}", said(&first));
    let pid = unit.await_pid();

    // Switching an existing unit to the product's executable is the same
    // second process reached in two steps.
    let switched = ensure_from(&fleet, &name, &program, "turn it into a vault helper");
    assert!(
        !switched.status.success(),
        "ensure switched a declared unit to a second skarbiec process"
    );
    assert!(
        said(&switched).contains("a second skarbiec process beside its one unit"),
        "got: {}",
        said(&switched)
    );
    assert_eq!(unit.live_pid(), Some(pid), "the refused pass touched the unit");
    assert_eq!(crate::only_record(&fleet)["program"], PROGRAM);

    // A declaration that already ran the executable before the rule existed
    // is the state every consolidation starts from, and it stays repairable
    // until its product absorbs it.
    let mut document = fleet.registry();
    document["targets"][0]["services"][0]["program"] = serde_json::json!(program);
    fleet.write(&document);
    let repaired = ensure_from(&fleet, &name, &program, "repair the unit it already ran");
    assert!(
        repaired.status.success(),
        "ensure refused to repair a unit that already ran the executable: {}",
        said(&repaired)
    );
    let printed = unit
        .printed()
        .expect("launchd holds no job under the label");
    assert!(
        printed.contains(&format!("program = {program}")),
        "launchd runs another program:\n{printed}"
    );
    assert_eq!(crate::only_record(&fleet)["program"], program.as_str());
}
