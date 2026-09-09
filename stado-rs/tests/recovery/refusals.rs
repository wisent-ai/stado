//! What this channel refuses, on this machine, with every sentence copied
//! from a live run of this fixture.
//!
//! Two of the three are about launchd's system domain, and they are the whole
//! reason this area exists: a unit under `/Library/LaunchDaemons` loads as
//! root, this login is not root, and the product has to say so before it
//! touches anything rather than reporting a repair it did not perform.
//! `/Library/LaunchDaemons` on this host is `root:wheel` and not writable by
//! this account (measured), so a case about a system daemon declares a path
//! there that nobody installed — which is exactly the state each case is
//! about — and then reads that nothing appeared at it.
//!
//! The third names a host the registry does not declare. That name is the
//! subject of the case, not a stand-in for a machine: the assertion is that
//! the command refuses a target it was never given.

use crate::fixture::fleet::{json_stdout, launchd_record, said, Fleet};
use crate::fixture::unit::{launchctl, Unit};

/// A host name no registry in this repository declares. It is the subject of
/// the last case: what a command does when the target does not exist.
const UNDECLARED_HOST: &str = "no-such-host-in-this-registry";

/// A fleet of this machine whose one declared service is a system
/// LaunchDaemon, at a path in the system domain that nothing installed.
fn declared_daemon(case: &str) -> (Fleet, String, String) {
    let label = format!(
        "{}stado-test.recovery-{}-{case}",
        stado::deploy::local_install::FLEET_LABEL_PREFIX,
        std::process::id()
    );
    let path = format!("/Library/LaunchDaemons/{label}.plist");
    assert!(
        !std::path::Path::new(&path).exists(),
        "{path} already exists; this case would be reading someone else's unit"
    );
    let fleet = Fleet::new();
    fleet.declare(launchd_record(&label, &path));
    (fleet, label, path)
}

/// A system daemon cannot be stopped without the host account, and this
/// fleet declares no credential for one. The refusal happens before the host
/// is contacted at all, which is what keeps a stop from half-running.
#[test]
fn stopping_a_system_daemon_is_refused_when_no_host_account_is_declared() {
    let (fleet, label, path) = declared_daemon("nopass");

    let out = fleet.stado(&["service", "stop", &label, "--host", &fleet.target, "--json"]);
    assert!(!out.status.success(), "a refusal is not a success");
    assert!(
        said(&out).contains(&format!(
            "{label} on {host} is a system LaunchDaemon and {host} has no readable host-account \
             password",
            host = fleet.target
        )),
        "got: {}",
        said(&out)
    );

    // And nothing was done to launchd's system domain on the way to saying so.
    let system = launchctl(&["print", &format!("system/{label}")]);
    assert!(
        !system.status.success(),
        "launchd's system domain holds {label}: {}",
        said(&system)
    );
    assert!(
        !std::path::Path::new(&path).exists(),
        "a refused stop left a unit file at {path}"
    );
}

/// A restart of a system daemon reads the unit file first, and a declaration
/// pointing at a file the host does not have is reported as that, with the
/// path named. This is the read-only half of the repair: nothing is
/// signalled, and no privileged command is attempted, because there is
/// nothing to signal.
#[test]
fn restarting_a_system_daemon_the_host_has_no_unit_file_for_reports_the_missing_path() {
    let (fleet, label, path) = declared_daemon("nofile");

    let out = fleet.stado(&[
        "service",
        "restart",
        &label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(!out.status.success(), "a refusal is not a success");
    assert!(
        said(&out).contains(&format!(
            "restart failed on {}: missing: {path}",
            fleet.target
        )),
        "got: {}",
        said(&out)
    );

    let report = &json_stdout(&out)[0];
    assert_eq!(report["unit"], label);
    assert_eq!(report["status"], "missing");
    assert_eq!(report["path"], path);
    assert_eq!(report["detail"], path);
    // The domain the path placed the unit in, and why loading it takes root.
    assert_eq!(report["launchd_domain"]["name"], "system");
    assert_eq!(report["launchd_domain"]["status"], "system");
    assert_eq!(
        report["launchd_domain"]["reason"],
        "a unit in /Library/LaunchDaemons is a system LaunchDaemon, so its job belongs to the \
         system domain and loading it needs root"
    );

    assert!(
        !std::path::Path::new(&path).exists(),
        "a refused restart created a unit file at {path}"
    );
}

/// A command aimed at a host nobody declared is refused by the registry's own
/// words, and the unit that IS declared is left exactly as it was — the pid
/// launchd holds is the same one before and after.
#[test]
fn a_host_the_registry_does_not_declare_is_refused_and_the_loaded_unit_is_untouched() {
    let fleet = Fleet::new();
    let unit = Unit::claim(
        &fleet,
        &format!(
            "{}stado-test.recovery-{}-unknown",
            stado::deploy::local_install::FLEET_LABEL_PREFIX,
            std::process::id()
        ),
    );
    unit.write_plist();
    fleet.declare(launchd_record(&unit.label, &unit.plist_arg()));
    let running = unit.load();
    let declared_before = fleet.registry_bytes();

    let out = fleet.stado(&[
        "service",
        "restart",
        &unit.label,
        "--host",
        UNDECLARED_HOST,
        "--json",
    ]);
    assert!(!out.status.success(), "a refusal is not a success");
    assert!(
        said(&out).contains(&format!(
            "target '{UNDECLARED_HOST}' is not in the canonical registry"
        )),
        "got: {}",
        said(&out)
    );

    assert_eq!(
        unit.live_pid(),
        Some(running),
        "the refused restart touched the unit launchd was holding"
    );
    assert_eq!(
        fleet.registry_bytes(),
        declared_before,
        "the refused restart rewrote the registry"
    );
}
