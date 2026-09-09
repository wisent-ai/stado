//! The host repair pass, run on this machine, over the managed health beacon
//! it is asked to reload.
//!
//! `stado repair stado --step host --target <this machine> --apply` is the
//! declared capability that runs `host_recovery::recover_host`. Because the
//! target's identity words are this machine's own kernel host name, the pass
//! executes here: it reads this host's `df -k /`, resolves this login's
//! launchd domain through the product's own resolver, and bootstraps the
//! beacon into that domain with the real `/bin/launchctl`.
//!
//! The beacon's label is fixed inside the product, so the unit guard is what
//! keeps `com.wisent.host-health-beacon` from outliving a case. Its plist
//! lives in the case's own isolated `HOME`, which is where the pass expands
//! `$HOME/Library/LaunchAgents/...` to.
//!
//! What `status: ok` is worth is the question this area was rebuilt to
//! answer, so every case reads two things: the report the command printed,
//! and launchd's own answer about the label afterwards.

use serde_json::Value;

use crate::fixture::fleet::{json_stdout, said, Fleet};
use crate::fixture::unit::{
    beacon_environment, beacon_lock, gui_domain, Unit, AMBIENT_CREDENTIAL_KEY, BEACON,
    BEACON_CONSUMER_KEY, BEACON_DECLARED_PATH,
};

/// A host name no registry in this repository declares. It is the subject of
/// the last case, not a stand-in for a machine.
const UNDECLARED_HOST: &str = "no-such-host-in-this-registry";

/// Apply the declared host repair step to this machine.
fn apply(fleet: &Fleet, target: &str) -> std::process::Output {
    fleet.stado(&[
        "repair", "stado", "--step", "host", "--target", target, "--apply", "--json",
    ])
}

/// The recovery document the step carries, out of the capability's report.
fn observation(out: &std::process::Output) -> Value {
    let report = json_stdout(out);
    let step = report["steps"][0].clone();
    assert_eq!(step["name"], "host", "the wrong step ran: {step}");
    step["observation"].clone()
}

/// Every case asserts the same three facts about the pass having really run
/// here: it named this host, and it recorded this host's free disk before and
/// after its own cleanup stage.
fn ran_on_this_host(observation: &Value, fleet: &Fleet) {
    assert_eq!(observation["target"], fleet.target);
    assert_eq!(observation["host"], fleet.target);
    for key in ["disk_free_kb_before", "disk_free_kb_after"] {
        let free = observation[key]
            .as_i64()
            .unwrap_or_else(|| panic!("{key} is not a number: {observation}"));
        assert!(free > 0, "{key} is {free}, which no mounted volume reports");
    }
    // No `$HOME/.stado/bin/stado` in an isolated home, so the pass has no
    // cleanup binary to run and says which stage it did not perform.
    assert_eq!(observation["cleanup_status"], "unavailable");
}

/// The one green pass: a beacon this login can load, in the domain the
/// product's own resolver chose, with nothing skipped and nothing blocking.
/// The evidence is launchd holding a job with a pid under the label.
#[test]
fn the_pass_reloads_the_declared_beacon_and_launchd_holds_the_job() {
    // Taken first, so it is released after the unit guard has cleared the
    // label: every case here addresses the product's one fixed beacon label.
    let _beacon_label = beacon_lock();
    let fleet = Fleet::new();
    let beacon = fleet.install_beacon(&beacon_environment());
    assert!(
        beacon.printed().is_none(),
        "launchd already holds {}",
        beacon.qualified()
    );

    let out = apply(&fleet, &fleet.target);
    assert!(out.status.success(), "the pass failed: {}", said(&out));

    // The state the pass left on this machine: a job under the label, from
    // the file it was told to load, running the program that file declares.
    let pid = beacon.await_pid();
    let printed = beacon
        .printed()
        .unwrap_or_else(|| panic!("launchd holds no job at {}", beacon.qualified()));
    assert!(
        printed.contains(&format!("path = {}", beacon.printed_path())),
        "launchd loaded some other file: {printed}"
    );
    assert!(
        printed.contains("state = running"),
        "launchd holds the job but is running nothing: {printed}"
    );
    assert!(pid > 0, "launchd reported pid {pid}");

    let observation = observation(&out);
    ran_on_this_host(&observation, &fleet);
    assert_eq!(observation["status"], "ok");
    assert_eq!(observation["agents"][BEACON], "restarted");
    assert_eq!(observation["skipped"], serde_json::json!([]));
    assert_eq!(observation["blockers"], serde_json::json!([]));
    assert_eq!(observation["exit_code"], 0);
    // The domain, and the reason the resolver chose it, in the pass's report:
    // a restart aimed at the other per-login domain does nothing at all.
    assert_eq!(observation["launchd_domain"]["name"], gui_domain());
    assert_eq!(observation["launchd_domain"]["status"], "graphical");
}

/// The finding behind twelve days of stale beacons: the declared unit file is
/// not on the host. It is a blocker of its own, the pass exits non-zero, and
/// the declaration's own `$HOME`-relative spelling is what the blocker names.
#[test]
fn a_declared_beacon_file_the_host_does_not_have_is_a_blocker() {
    let _beacon_label = beacon_lock();
    let fleet = Fleet::new();
    // Nothing is written under the isolated HOME: the case is the absence.
    let beacon = Unit::claim(&fleet, BEACON);

    let out = apply(&fleet, &fleet.target);
    assert!(
        !out.status.success(),
        "a pass that loaded no managed unit is not a success: {}",
        said(&out)
    );
    assert!(
        said(&out).contains(&format!(
            "{} did not complete host repair; inspect the reported blockers and retry the \
             declared stado host step.",
            fleet.target
        )),
        "got: {}",
        said(&out)
    );

    let observation = observation(&out);
    ran_on_this_host(&observation, &fleet);
    assert_eq!(observation["status"], "blocked");
    assert_eq!(observation["agents"][BEACON], "missing_plist");
    assert_eq!(observation["skipped"], serde_json::json!([]));
    assert_eq!(
        observation["blockers"],
        serde_json::json!([{
            "unit": BEACON,
            "finding": "missing_plist",
            "path": BEACON_DECLARED_PATH,
            "reason": format!(
                "the declared unit file {BEACON_DECLARED_PATH} is not on the host, so there is \
                 nothing to load and this host publishes no beacon. Reinstall it and load it \
                 with: sudo launchctl bootstrap system {BEACON_DECLARED_PATH}"
            ),
        }]),
        "the blocker must name the file, the consequence and the command that fixes it"
    );

    // And launchd holds nothing under the label, which is the state the
    // blocker describes.
    assert!(
        beacon.printed().is_none(),
        "launchd holds {} after a pass that found no unit file",
        beacon.qualified()
    );
}

/// The beacon publishes host health with a scoped Skarbiec grant, and the
/// pass validates that configuration before it loads the unit. A beacon
/// missing part of it is refused, and the proof is that launchd is holding
/// nothing: the pass stopped before `bootstrap`.
#[test]
fn a_beacon_whose_scoped_health_configuration_is_incomplete_is_not_loaded() {
    let _beacon_label = beacon_lock();
    let fleet = Fleet::new();
    let mut environment = beacon_environment();
    let consumer = environment
        .iter()
        .position(|(name, _)| *name == BEACON_CONSUMER_KEY)
        .expect("the beacon declares its Skarbiec consumer");
    environment.remove(consumer);
    let beacon = fleet.install_beacon(&environment);

    let out = apply(&fleet, &fleet.target);
    assert!(!out.status.success(), "a refusal is not a success");
    let observation = observation(&out);
    ran_on_this_host(&observation, &fleet);
    assert_eq!(observation["status"], "blocked");
    assert_eq!(
        observation["agents"][BEACON],
        "invalid_scoped_health_config"
    );
    assert_eq!(
        observation["blockers"],
        serde_json::json!([{
            "unit": BEACON,
            "finding": "invalid_scoped_health_config",
            "path": BEACON_DECLARED_PATH,
            "domain": gui_domain(),
            "reason": format!(
                "the recovery pass refused to load {BEACON} from {BEACON_DECLARED_PATH}; the \
                 finding is its own word for why, and the unit is not running"
            ),
        }])
    );
    assert!(
        beacon.printed().is_none(),
        "the pass loaded {} despite refusing its configuration",
        beacon.qualified()
    );
}

/// An ambient Google credential in the unit's environment is authority the
/// beacon must not have, and a pass that loaded it would put it back. It is
/// refused by name, and launchd is left holding nothing.
#[test]
fn a_beacon_carrying_an_ambient_credential_is_refused_before_it_is_loaded() {
    let _beacon_label = beacon_lock();
    let fleet = Fleet::new();
    let mut environment = beacon_environment();
    environment.push((AMBIENT_CREDENTIAL_KEY, "/dev/null"));
    let beacon = fleet.install_beacon(&environment);

    let out = apply(&fleet, &fleet.target);
    assert!(!out.status.success(), "a refusal is not a success");
    let observation = observation(&out);
    ran_on_this_host(&observation, &fleet);
    assert_eq!(observation["status"], "blocked");
    assert_eq!(
        observation["agents"][BEACON],
        "forbidden_ambient_health_credential"
    );
    assert_eq!(
        observation["blockers"][0]["finding"],
        "forbidden_ambient_health_credential"
    );
    assert!(
        beacon.printed().is_none(),
        "the pass loaded {} while refusing its ambient credential",
        beacon.qualified()
    );
}

/// A target the registry does not declare is a wrong request, not an
/// observation about a host: the pass never runs, and the command says which
/// name it could not resolve.
#[test]
fn a_target_the_registry_does_not_declare_is_refused_before_any_pass_runs() {
    let _beacon_label = beacon_lock();
    let fleet = Fleet::new();
    let beacon = fleet.install_beacon(&beacon_environment());

    let out = apply(&fleet, UNDECLARED_HOST);
    assert!(!out.status.success(), "a refusal is not a success");
    assert!(
        said(&out).contains(&format!(
            "target '{UNDECLARED_HOST}' is not in the canonical registry"
        )),
        "got: {}",
        said(&out)
    );
    // The declared beacon of the host that IS in this registry was not
    // touched: nothing was loaded for a target that does not exist.
    assert!(
        beacon.printed().is_none(),
        "a refused target still loaded {}",
        beacon.qualified()
    );
}
