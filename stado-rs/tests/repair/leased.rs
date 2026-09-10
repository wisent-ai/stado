//! Declared repair reported against a target this case owns outright.
//!
//! The cases beside this one drive the real binary against THIS machine, and
//! that is the right host for a read: the observation they check is the
//! operator's own box. What no read of the operator's box can answer is what
//! the capability reports about a machine whose entire state belongs to the
//! case, so this file leases one. `stado scratch create` makes a throwaway
//! account on a registry host the fleet itself declares leasable and emits a
//! registry naming exactly that target; the case adds its own service
//! declaration to that document — a service whose program lives under the
//! leased account's home — drives the capability through it, reads the
//! machine back through a second command, and destroys the lease with the
//! three absences asserted.
//!
//! Nothing is applied, and the declaration says why. Every repair step in
//! `data/catalog/service-catalog.json` is `mutating`, and every one of them restores
//! a service the operator owns: the Stado host program, the core object API,
//! the release store and its verifier grants, the control plane's delivered
//! binaries, Skarbiec's audit journal, keybox and acquisition state. Not one
//! can be applied entirely inside a leased account's own state, and several
//! would have to write to Skarbiec or to the release channel to run at all.
//! So the mutating half is covered by the two refusals the capability owns,
//! each by its exact sentence, and
//! `every_declared_repair_step_mutates_a_service_the_operator_owns` is the
//! case that fails the day a step arrives which a lease could apply.

mod harness;

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use super::fixture::{stderr, stdout, DECLARATION, SERVICE};
use harness::{
    assert_absences, declared_steps, document, leased, take_lease, DECLARED_TREE, DECLARED_VERSION,
    HOST, REFUSED,
};

/// The reads that say which account answered, and what that account's home
/// holds where the declaration says its service is installed.
fn assert_leased_account(lease: &harness::Lease) {
    let who = lease.probe(&["id"]);
    assert!(
        who["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!("({})", lease.name)),
        "the account on the far side of the channel is the lease: {who}"
    );
    let tree = lease.probe(&["ls", "-l", DECLARED_TREE]);
    assert_eq!(
        tree["exit_code"],
        json!(REFUSED),
        "the declared program's tree is absent from the leased home: {tree}"
    );
    assert!(
        tree["stderr"]
            .as_str()
            .unwrap_or_default()
            .contains("No such file or directory"),
        "{tree}"
    );
}

/// What the leased machine said about itself, inside the step reports.
fn assert_observation(lease: &harness::Lease, seen: &Value) {
    assert_eq!(seen["target"], json!(lease.name.clone()));
    assert_eq!(seen["ssh"], json!(lease.ssh.clone()));
    assert_eq!(seen["status"], "inventory");
    assert_eq!(
        seen["sanitizer_state"], "ok",
        "the remote probe checked its own sanitizer before reporting: {seen}"
    );
    assert_eq!(seen["release_platform"], json!(lease.platform.clone()));
    assert_eq!(seen["release_platform_verdict"], "matched");

    // The declaration this case wrote, beside what the machine holds against
    // it. The operator's own account on the same box carries an installed
    // Stado; the leased account carries none, so this pair is also the proof
    // that the home read was the lease's.
    let managed = seen["managed_binaries"]
        .as_array()
        .expect("the observation lists managed binaries")
        .iter()
        .find(|binary| binary["name"] == "stado")
        .expect("the managed inventory covers the Stado binary")
        .clone();
    assert_eq!(
        managed["declared_version"], DECLARED_VERSION,
        "the declared version in the report is the one this case wrote"
    );
    assert_eq!(
        managed["state"], "missing",
        "the leased account holds no Stado of its own: {managed}"
    );
    assert_eq!(
        seen["service_artifacts"],
        json!([]),
        "the declared service is installed nowhere in the leased home: {seen}"
    );
    assert_eq!(seen["cargo"]["home"]["kind"], "missing");
    assert_eq!(seen["forwards_dir_state"], "missing");
    assert_eq!(seen["vaults"], json!([]));
    for subcommand in seen["subcommands"].as_array().into_iter().flatten() {
        assert_eq!(
            subcommand["state"], "unavailable",
            "a fresh lease runs no Stado subcommand of its own: {subcommand}"
        );
    }
}

#[test]
fn a_declared_repair_is_reported_against_a_leased_target_from_its_own_registry() {
    let _turn = HOST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut lease = take_lease();
    lease.declare_service();

    // The declaration side, read through a registry that declares one machine
    // and one service of its own, and answered out of the compiled catalogue
    // regardless — `repair list` and `repair show` read the catalogue, never
    // the registry they were pointed at.
    let steps = declared_steps(&lease.root, SERVICE);
    assert!(!steps.is_empty(), "the service declares repair steps");
    for step in &steps {
        let name = step["name"].as_str().expect("a step name");
        let arguments = ["repair", "show", SERVICE, name, "--json"];
        let shown = leased(&lease.root, &arguments);
        let report = document(&shown, &arguments);
        assert_eq!(report["declaration"], DECLARATION);
        assert_eq!(report["service"], SERVICE);
        assert_eq!(
            report["step"], *step,
            "`repair show` and `repair list` disagree about {name}"
        );
    }

    // The dry run, against the leased target, through the lease's document.
    let arguments = ["repair", SERVICE, "--target", &lease.name, "--json"];
    let reported = leased(&lease.root, &arguments);
    let report = document(&reported, &arguments);
    assert_eq!(report["declaration"], DECLARATION);
    assert_eq!(report["applied"], json!(false));
    assert_eq!(report["target"], json!(lease.name.clone()));
    let reported_steps = report["steps"].as_array().expect("reported steps");
    assert_eq!(
        reported_steps.len(),
        steps.len(),
        "the run reports every declared step: {report}"
    );
    for (reported, declared) in reported_steps.iter().zip(&steps) {
        assert_eq!(reported["name"], declared["name"]);
        assert_eq!(reported["proof"], declared["proof"]);
        assert_eq!(reported["summary"], declared["summary"]);
        assert_eq!(reported["mutating"], declared["mutating"]);
        assert_eq!(reported["status"], "planned", "nothing ran: {reported}");
        assert_eq!(
            reported["observation"], reported_steps[0]["observation"],
            "one reading of one machine covers the whole plan: {reported}"
        );
    }

    assert_observation(&lease, &reported_steps[0]["observation"]);
    // The same home, read by a different capability over the same channel.
    assert_leased_account(&lease);

    let destroyed = lease.release();
    assert_absences(&destroyed);
    assert!(
        !Path::new(&lease.root).exists(),
        "the emitted registry went with the lease"
    );
}

#[test]
fn every_declared_repair_step_mutates_a_service_the_operator_owns() {
    let _turn = HOST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut lease = take_lease();
    lease.declare_service();

    // Why nothing is applied here: every declared step mutates, and every one
    // repairs a fleet service the operator owns, so none can be run entirely
    // inside a leased account's own state.
    let arguments = ["repair", "list", "--json"];
    let listed = leased(&lease.root, &arguments);
    let catalogue = document(&listed, &arguments);
    let mut declared = usize::MIN;
    for service in catalogue["services"].as_array().into_iter().flatten() {
        for step in service["repair"].as_array().into_iter().flatten() {
            assert_eq!(
                step["mutating"],
                json!(true),
                "a step a lease could apply arrived; give it the applied leg: {} {step}",
                service["name"]
            );
            declared += 1;
        }
    }
    assert!(declared > usize::MIN, "the catalogue declares repair steps");

    // A declared step whose implementation is not there is refused, and a
    // real leased target on the command line does not change the answer.
    let missing = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["repair", SERVICE, "--target", &lease.name])
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", &lease.root)
        .env(
            "STADO_CONFIG",
            Path::new(&lease.root).join("no-config.json"),
        )
        .env("STADO_REPAIR_TEST_MISSING_IMPLEMENTATION", "stado:host")
        .output()
        .expect("the built stado binary runs");
    assert_eq!(missing.status.code(), Some(REFUSED), "{}", stderr(&missing));
    assert!(
        stderr(&missing).contains(
            "stado repair step host declares no implementation; \
             add it to stado-rs/src/cli/repair/steps.rs."
        ),
        "{}",
        stderr(&missing)
    );
    assert!(
        stdout(&missing).is_empty(),
        "a refused run reports nothing: {}",
        stdout(&missing)
    );

    // A mutating step with no target selected is refused before anything runs.
    let untargeted = leased(&lease.root, &["repair", SERVICE, "--apply"]);
    assert_eq!(
        untargeted.status.code(),
        Some(REFUSED),
        "{}",
        stderr(&untargeted)
    );
    assert!(
        stderr(&untargeted).contains(
            "stado declares mutating repair steps but no target was selected; \
             pass --target <TARGET>."
        ),
        "{}",
        stderr(&untargeted)
    );
    assert!(
        stdout(&untargeted).is_empty(),
        "a refused run reports nothing: {}",
        stdout(&untargeted)
    );

    // And the machine is as it was: the refusals mutated nothing on it.
    assert_leased_account(&lease);
    assert_absences(&lease.release());
}
