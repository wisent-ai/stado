//! `stado placement role take`: a role of the host Stado process taken on
//! this host from an active host that does not answer, run against an
//! isolated deployment's own registry and the built `stado`.
//!
//! The take is committed by the host that holds the registry authority with
//! the registry it keeps itself, so its refusals are about what it is taken
//! with: a host that keeps no last-known-good copy has no registry to take
//! the role with, and says so before it reads anything else. A refusal
//! writes nothing to the registry.

use crate::deployment::Deployment;

const ROLE: &str = "stado-object-api";

#[test]
fn a_role_is_not_taken_without_this_hosts_registry_copy() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();
    // The isolated deployment's resolver never read the registry, so it keeps
    // no last-known-good copy.
    for check in [true, false] {
        let mut args = vec!["placement", "role", "take", ROLE, "--reason", "journey"];
        if check {
            args.push("--check");
        }
        deployment.refused(&args, "this host keeps no last-known-good registry");
    }
}
