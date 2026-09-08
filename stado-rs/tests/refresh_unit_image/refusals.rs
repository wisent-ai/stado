//! Every refusal the verb makes, against a real loaded unit on this machine.
//!
//! The load-bearing assertion in the first two cases is the last one: the pid
//! launchd holds for the label is the pid it held before the command ran. A
//! refusal that restarted first would be no refusal at all, and that is read
//! off launchd rather than inferred from the sentence.

use stado::deploy::service::IMAGE_SETTLE_SECONDS;

use crate::host::{digest, said, Host};
use crate::unit::{label, write_plist, LoadedUnit};

/// A unit executing the file it declares is left alone, and the refusal names
/// what was found.
#[test]
fn a_unit_that_is_not_stale_is_refused_and_left_running() {
    let host = Host::new();
    let unit = LoadedUnit::start(&host, "healthy");
    let running = unit.image();
    assert_eq!(
        digest(std::path::Path::new(&running.path)),
        unit.digest,
        "the process must be executing the bytes this test placed at its declared path"
    );

    let output = host.refresh(&unit.label);
    let said = said(&output);
    assert!(
        !output.status.success(),
        "a healthy unit must exit non-zero to refuse: {said}"
    );
    assert!(
        said.contains("is not stale and was not restarted"),
        "the refusal must say it refused: {said}"
    );
    assert!(
        said.contains(&running.describe()),
        "the refusal must name the identity it found: {said}"
    );
    assert!(
        said.contains("Restarting it would be an outage with nothing to fix"),
        "the refusal must say why it is a refusal and not a caveat: {said}"
    );
    assert_eq!(
        unit.live_pid(),
        Some(unit.pid),
        "the process must still be the one that was running: a refusal that restarts first is a \
         restart button"
    );
}

/// A replacement younger than the settle window is an installer mid-flight.
#[test]
fn a_replacement_still_in_flight_is_refused() {
    let host = Host::new();
    let unit = LoadedUnit::start(&host, "inflight");
    let replacement = unit.replace_image();
    assert_ne!(
        replacement, unit.digest,
        "the replacement must be different bytes, or this proves nothing"
    );

    let output = host.refresh(&unit.label);
    let said = said(&output);
    assert!(
        !output.status.success(),
        "mid-flight must exit non-zero: {said}"
    );
    assert!(
        said.contains(&format!("less than {IMAGE_SETTLE_SECONDS}s ago")),
        "the refusal must say the replacement is too young: {said}"
    );
    assert!(
        said.contains("re-run once it has settled"),
        "the refusal must say what to do instead: {said}"
    );
    assert_eq!(
        unit.live_pid(),
        Some(unit.pid),
        "nothing may be restarted mid-flight"
    );
}

/// An identity that could not be read is not a reason to act.
///
/// A declaration naming no program at all: the file is real, the product
/// really parses it, and no process is looked for because there is nothing to
/// compare one against.
#[test]
fn an_unread_unit_is_refused_rather_than_restarted() {
    let host = Host::new();
    let label = label("programless");
    let plist = write_plist(&host, &label, &[], false);

    let output = host.refresh(&label);
    let said = said(&output);
    assert!(
        !output.status.success(),
        "unread must exit non-zero: {said}"
    );
    assert!(
        said.contains("whether it is stale is unknown"),
        "the refusal must name the unknown rather than acting on it: {said}"
    );
    assert!(
        said.contains(
            "An unread state is not a reason to act any more than it is a reason to \
                       pass"
        ),
        "the refusal must say why an unread state is not permission: {said}"
    );
    assert!(
        said.contains(&plist.display().to_string()),
        "the refusal must name the file it could not read a program from: {said}"
    );
}

/// A label this machine holds no running unit for is refused with the reason.
///
/// The unit file is real and declares a real executable; what is missing is a
/// loaded job, because nothing bootstrapped it.
#[test]
fn a_unit_with_no_live_process_is_refused_with_the_reason() {
    let host = Host::new();
    let label = label("notloaded");
    let program = host.root.join("bin/notloaded");
    std::fs::copy(crate::unit::current_exe(), &program).expect("place the declared executable");
    write_plist(&host, &label, &[program.display().to_string()], true);

    let output = host.refresh(&label);
    let said = said(&output);
    assert!(
        !output.status.success(),
        "an unknown unit must exit non-zero: {said}"
    );
    assert!(
        said.contains("holds no launchd unit named"),
        "the refusal must say what it looked for: {said}"
    );
    assert!(
        said.contains("a job that is not running holds no image"),
        "the refusal must say why that is not a fault: {said}"
    );
    assert!(
        said.contains(&host.target),
        "the refusal must name this machine: {said}"
    );
}

/// A registry that names no machine is refused before anything is read.
///
/// This is the honest way to reach that refusal: a document with no target at
/// all. The deleted version of this case declared a target for a machine that
/// does not exist, which is exactly the invented host constant this area no
/// longer contains.
#[test]
fn a_registry_that_names_no_machine_is_refused_before_it_looks() {
    let host = Host::new();
    host.declare_no_machine();
    let label = label("nomachine");

    let output = host.refresh(&label);
    let said = said(&output);
    assert!(!output.status.success(), "must exit non-zero: {said}");
    assert!(
        said.contains("no registry target names this machine"),
        "the refusal must say the registry does not name this host: {said}"
    );
    assert!(
        said.contains("readable only on the machine holding that process"),
        "the refusal must say the read is local: {said}"
    );
    assert!(
        said.contains(&crate::host::hostname()),
        "the refusal must name the machine it is running on: {said}"
    );
}
