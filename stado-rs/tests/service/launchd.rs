//! What the product does to, and reads off, a unit launchd is really holding.
//!
//! The three verbs here were the ones the old suite could say least about: it
//! asserted `restart` against a shell script that appended `restart <unit>`
//! to a log file, and `logs` against a script that printed one invented line.
//! Neither could observe the thing that actually goes wrong — a per-login job
//! addressed in the wrong domain, or a tail read from a file the unit does
//! not declare. Here the unit is loaded in this login's own `gui/<uid>`
//! domain, and each case checks the domain the product chose against the
//! domain launchd put the job in.

use crate::fixture::fleet::{json_stdout, said, Fleet};
use crate::fixture::unit::{self, Unit, PROGRAM};
use crate::{ensure, only_record};

/// A non-secret name and a credential-looking name, declared together: the
/// environment readback must show the first and redact the second.
const MARKER: &str = "STADO_SERVICE_AREA_MARKER";
const MARKER_VALUE: &str = "area-value";
const SECRET: &str = "STADO_SERVICE_AREA_API_TOKEN";
const SECRET_VALUE: &str = "area-token-value";

#[test]
fn restart_addresses_this_logins_own_domain_and_launchd_holds_the_pid_reported() {
    let fleet = Fleet::lifecycle();
    let name = unit::service_name("restart");
    let unit = Unit::claim(&fleet, &name);
    let installed = ensure(&fleet, &name, "install the unit this case restarts");
    assert!(
        installed.status.success(),
        "ensure failed: {}",
        said(&installed)
    );
    let before = unit.await_pid();

    let out = fleet.stado(&[
        "service",
        "restart",
        &unit.label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(out.status.success(), "restart failed: {}", said(&out));
    let report = &json_stdout(&out)[0];
    assert_eq!(report["host"], fleet.target);
    assert_eq!(report["os"], "Darwin");
    assert_eq!(report["status"], "restarted");
    assert_eq!(report["path"], unit.plist_arg());
    // The domain is the whole point: a LaunchAgent of this login belongs to
    // `gui/<uid>`, and a restart sent anywhere else silently does nothing.
    assert_eq!(report["launchd_domain"]["name"], unit.domain);
    assert_eq!(report["launchd_domain"]["status"], "graphical");
    assert_eq!(report["postcondition"]["state"], "met");

    let pid = unit.await_pid();
    assert_eq!(
        report["postcondition"]["detail"],
        format!("{} pid {pid}", unit.qualified()),
        "the postcondition names a pid launchd does not hold"
    );
    assert_ne!(before, pid, "the process was never replaced");
}

#[test]
fn the_log_tail_is_read_from_the_file_the_installed_unit_declares() {
    let fleet = Fleet::lifecycle();
    let name = unit::service_name("logs");
    let unit = Unit::claim(&fleet, &name);
    let installed = ensure(&fleet, &name, "install the unit whose log this case tails");
    assert!(
        installed.status.success(),
        "ensure failed: {}",
        said(&installed)
    );

    // The path is not chosen by this test: it is the one the plist the
    // product wrote declares, read back out of that file.
    let declared = std::fs::read_to_string(&unit.plist).expect("the installed plist is readable");
    let log = fleet.logs().join(format!("{}.log", unit.label));
    assert!(
        declared.contains(&log.display().to_string()),
        "the unit declares another log path:\n{declared}"
    );

    let out = fleet.stado(&[
        "service",
        "logs",
        &unit.label,
        "--host",
        &fleet.target,
        "--lines",
        "20",
        "--json",
    ]);
    assert!(out.status.success(), "logs failed: {}", said(&out));
    let report = &json_stdout(&out)[0];
    assert_eq!(report["host"], fleet.target);
    assert_eq!(report["unit"], unit.label);
    assert_eq!(
        report["origin"],
        log.display().to_string(),
        "the tail came from somewhere other than the declared file"
    );
    // `/bin/sleep` writes nothing, and the reader says which file was empty
    // rather than inventing a line.
    assert!(
        said(&out).contains(&format!("{} (empty)", log.display())),
        "the empty declared file was not named: {}",
        said(&out)
    );
}

#[test]
fn the_declared_environment_reaches_the_unit_and_reads_back_with_credentials_redacted() {
    let fleet = Fleet::lifecycle();
    let name = unit::service_name("env");
    let unit = Unit::claim(&fleet, &name);

    let installed = fleet.stado(&[
        "service",
        "ensure",
        &name,
        "--host",
        &fleet.target,
        "--from",
        PROGRAM,
        "--arg",
        unit::HOLD_SECONDS,
        "--env",
        &format!("{MARKER}={MARKER_VALUE}"),
        "--env",
        &format!("{SECRET}={SECRET_VALUE}"),
        "--reason",
        "prove declared variables reach the installed unit",
        "--json",
    ]);
    assert!(
        installed.status.success(),
        "ensure failed: {}",
        said(&installed)
    );

    // Both variables are really in the unit file, secret included: the
    // redaction is a property of the report, not of the installation.
    let plist = std::fs::read_to_string(&unit.plist).expect("the installed plist is readable");
    for (name, value) in [(MARKER, MARKER_VALUE), (SECRET, SECRET_VALUE)] {
        assert!(
            plist.contains(name) && plist.contains(value),
            "{name} never reached the unit file:\n{plist}"
        );
    }

    let out = fleet.stado(&[
        "service",
        "env",
        &unit.label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(out.status.success(), "env failed: {}", said(&out));
    let report = &json_stdout(&out)[0];
    assert_eq!(report["kind"], "launchd");
    assert_eq!(report["path"], unit.plist_arg());
    let environment = &report["environment"];
    assert_eq!(environment[MARKER], MARKER_VALUE);
    assert_eq!(
        environment[SECRET], "[REDACTED]",
        "a credential-looking variable was printed in full"
    );
    assert!(
        !said(&out).contains(SECRET_VALUE),
        "the secret's value reached the operator's terminal"
    );
    // HOME is the unit's own, inside this case's tempdir — the isolation the
    // whole area rests on, stated by the product rather than by this test.
    assert_eq!(environment["HOME"], fleet.home.display().to_string());

    // The record the pass persisted names the same variables it installed.
    let record = only_record(&fleet);
    assert_eq!(record["env"][MARKER], MARKER_VALUE);
    assert_eq!(record["env"][SECRET], SECRET_VALUE);
}

/// The init system's own always-loaded unit, in the domain only the system
/// manager holds: macOS keeps `opendirectoryd` in `system`, systemd keeps the
/// journal there. Both are readable without privilege.
fn system_unit() -> &'static str {
    if cfg!(target_os = "macos") {
        "com.apple.opendirectoryd"
    } else {
        "systemd-journald.service"
    }
}

/// Reading a domain needs no privilege, and asking for one turned a readable
/// daemon into an unreadable one.
///
/// Before this case the program read the system domain through
/// `sudo -n launchctl print`. On a host whose channel has no passwordless
/// sudo — every Mac in this fleet — that read came back
/// "sudo: a password is required", so `stado service label-print` answered
/// `unavailable` for a daemon launchd was holding, and the beacon collector
/// built on the same idea published `inactive` for it.
#[test]
fn a_system_domain_unit_is_read_without_borrowing_root() {
    let fleet = Fleet::lifecycle();
    let label = system_unit();
    let out = fleet.stado(&[
        "service",
        "label-print",
        label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(
        out.status.success(),
        "label-print could not read {label}: {}",
        said(&out)
    );
    let report = json_stdout(&out);
    assert_eq!(report["read_status"], "loaded", "{}", said(&out));
    assert_eq!(report["domain"], "system", "{}", said(&out));
    assert!(
        report["program"].as_str().is_some_and(|p| !p.is_empty()),
        "the read produced no program for a loaded daemon: {}",
        said(&out)
    );
    for failure in report["read_failures"].as_array().into_iter().flatten() {
        assert_ne!(
            failure["kind"],
            "permission_refused",
            "a domain read was refused for want of privilege: {}",
            said(&out)
        );
    }
}

/// A unit no init system holds is absent, and absence is not a refusal.
#[test]
fn a_unit_no_init_system_holds_is_absent_and_says_so() {
    let fleet = Fleet::lifecycle();
    let label = format!("com.wisent.service-area.absent.{}", std::process::id());
    let out = fleet.stado(&[
        "service",
        "label-print",
        &label,
        "--host",
        &fleet.target,
        "--json",
    ]);
    assert!(
        !out.status.success(),
        "label-print claimed to find {label}: {}",
        said(&out)
    );
    let report = json_stdout(&out);
    assert_eq!(report["read_status"], "not_loaded", "{}", said(&out));
    assert_eq!(
        report["read_failures"].as_array().map(Vec::len),
        Some(0),
        "an absent unit was reported with a failed read: {}",
        said(&out)
    );
    let spoken = said(&out);
    assert!(
        spoken.contains(&format!("{label} is not loaded")),
        "the refusal did not say the unit is absent: {spoken}"
    );
    assert!(
        !spoken.contains("cannot tell whether"),
        "an absent unit was reported as unreadable: {spoken}"
    );
}
