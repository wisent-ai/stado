//! The refusals: an unregistered host, an ambiguous identity, a malformed
//! declaration, and a host declared for a platform this machine is not.
//!
//! Every one of them is reached before any connection is opened, so they are
//! answers about declared data and nothing else. The two cases that ask what
//! this machine really is answer through the product's own host inventory,
//! which on the current host runs this machine's own tools.

use serde_json::json;

use crate::fixture::Policy;
use crate::{held_port, hostname, platform, report, said, stderr, stdout, Host, SERVICE, TARGET};

const GENERATION: u64 = 7;
/// A platform the product publishes nothing for.
const UNPUBLISHED_PLATFORM: &str = "windows-arm64";

/// The published release platform this machine is not.
fn other_platform() -> &'static str {
    if platform() == "darwin-arm64" {
        "linux-amd64"
    } else {
        "darwin-arm64"
    }
}

/// A host identity that is emphatically not this machine's, derived from this
/// machine's own name so no invented hostname can accidentally match it.
fn not_this_host() -> String {
    format!("not-{}", hostname())
}

#[test]
fn a_target_the_registry_does_not_carry_and_one_with_no_resolver_policy_are_both_refused() {
    let policy = Policy::patient(GENERATION);
    let host = Host::new(&policy.document());

    let answer = host.stado(&["resolver", "status", "--target", "ghost"]);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    assert!(
        stderr(&answer).contains("Error: resolver target \"ghost\" is not registered"),
        "got: {}",
        said(&answer)
    );
    assert!(
        stdout(&answer).is_empty(),
        "a refused status printed a report anyway: {}",
        said(&answer)
    );

    // A registered host that declares no resolver policy is refused by that
    // fact, not answered with an empty report.
    let mut silent = policy.document();
    silent["targets"][0]
        .as_object_mut()
        .unwrap()
        .remove("service_resolver");
    host.write_registry(&silent);
    let answer = host.stado(&["resolver", "status", "--target", TARGET]);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    assert!(
        stderr(&answer).contains("Error: registry target has no service_resolver configuration"),
        "got: {}",
        said(&answer)
    );
    assert!(
        stdout(&answer).is_empty(),
        "a refused status printed a report anyway: {}",
        said(&answer)
    );
}

#[test]
fn two_targets_claiming_this_machines_identity_are_refused_and_both_are_named() {
    let policy = Policy::patient(GENERATION);
    let mut ambiguous = policy.document();
    let twin = format!("{TARGET}-twin");
    let mut second = policy.target(&hostname());
    second["name"] = json!(twin);
    second.as_object_mut().unwrap().remove("service_resolver");
    // The second entry declares no connection path of its own: the identity
    // under test is the hostname both entries claim, not a second one.
    second.as_object_mut().unwrap().remove("ssh");
    ambiguous["targets"] = json!([policy.target(&hostname()), second]);
    let host = Host::new(&ambiguous);

    let answer = host.stado(&[
        "registry",
        "validate",
        host.registry_path().to_str().unwrap(),
    ]);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    assert!(
        stderr(&answer).contains(&format!(
            "Error: registry.targets[1].hostnames[0]: host identity '{}' is already declared by \
             registry.targets[0].hostnames[0]",
            hostname()
        )),
        "got: {}",
        said(&answer)
    );

    // And the resolver, asked which host it is on, refuses to pick one.
    let answer = host.stado(&["resolver", "status"]);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    assert!(
        stderr(&answer).contains(&format!(
            "Error: hostname '{}' matches multiple registry targets: {TARGET}, {twin}",
            hostname()
        )),
        "got: {}",
        said(&answer)
    );
}

#[test]
fn a_registry_that_names_no_host_here_never_reports_this_machines_sockets_as_another_hosts() {
    // The declared binds are held right here, and declared over there. That
    // is the collision the report must not resolve by dialling: a loopback
    // address is an address *on the target*, and answering `listening: true`
    // about this machine's socket would vouch for a host nobody measured.
    let (api_socket, api) = held_port();
    let (adapter_socket, adapter) = held_port();
    let mut policy = Policy::patient(GENERATION);
    policy.api = api;
    policy.adapter = adapter;
    let host = Host::new(&policy.document_for(&not_this_host()));

    let answer = host.stado(&["resolver", "status", "--target", TARGET, "--json"]);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    let elsewhere = report(&answer);
    assert_eq!(
        elsewhere["api"]["listening"],
        serde_json::Value::Null,
        "the API bind was reported from a socket this test holds: {elsewhere}"
    );
    assert_eq!(
        elsewhere["adapters"][0]["listening"],
        serde_json::Value::Null,
        "the adapter bind was reported from a socket this test holds: {elsewhere}"
    );
    assert_eq!(
        elsewhere["bind_probe"],
        format!(
            "not probed: these binds are loopback addresses on {TARGET}, and this command ran on \
             a host with no registry identity; ask that host with `stado host inventory {TARGET}`"
        )
    );
    assert_ne!(
        elsewhere["verdict"], "down",
        "an unprobed bind was read as a dead resolver: {elsewhere}"
    );

    // Resolution itself refuses rather than guessing which target this is.
    let answer = host.stado(&[
        "resolver",
        "resolve",
        SERVICE,
        "--consumer",
        crate::CONSUMER,
    ]);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    assert!(
        stderr(&answer).to_ascii_lowercase().contains(&format!(
            "error: resolver host \"{}\" has no registry target identity",
            hostname()
        )),
        "got: {}",
        said(&answer)
    );

    drop((api_socket, adapter_socket));
}

#[test]
fn a_host_this_machine_is_not_and_cannot_reach_is_refused_before_a_connection_is_opened() {
    let policy = Policy::patient(GENERATION);
    let mut fleet = policy.document();
    let elsewhere = "resolver-other-host";
    // A second registered host: not this machine, and declaring no connection
    // path, which is what makes reaching it impossible from here.
    fleet["targets"] = json!([
        policy.target(&hostname()),
        {
            "name": elsewhere,
            "kind": "local",
            "release_platform": other_platform(),
            "hostnames": [not_this_host()],
            "services": [],
        },
    ]);
    let host = Host::new(&fleet);

    let answer = host.stado(&[
        "registry",
        "validate",
        host.registry_path().to_str().unwrap(),
    ]);
    assert!(
        answer.status.success(),
        "the two-host document is not valid: {}",
        said(&answer)
    );

    let answer = host.stado(&["host", "inventory", elsewhere]);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    assert!(
        stderr(&answer).contains(&format!(
            "Error: target '{elsewhere}' has no registry-managed ssh destination and is not this \
             host"
        )),
        "got: {}",
        said(&answer)
    );

    // The same command against the target that IS this machine runs here and
    // answers about this machine.
    let answer = host.stado(&["host", "inventory", TARGET, "--json"]);
    assert!(answer.status.success(), "{}", said(&answer));
    let inventory = report(&answer);
    assert_eq!(inventory["target"], TARGET);
    assert_eq!(inventory["sanitizer_state"], "ok");
    assert_eq!(inventory["release_platform"], platform());
    assert_eq!(inventory["declared_release_platform"], platform());
    assert_eq!(inventory["release_platform_verdict"], "matched");
}

#[test]
fn a_host_declared_for_another_platform_is_refused_or_contradicted_by_this_machine() {
    let policy = Policy::patient(GENERATION);

    // A platform the product publishes nothing for is refused by the
    // document's own validator, with the published set named.
    let mut unpublished = policy.document();
    unpublished["targets"][0]["release_platform"] = json!(UNPUBLISHED_PLATFORM);
    let host = Host::new(&unpublished);
    let answer = host.stado(&[
        "registry",
        "validate",
        host.registry_path().to_str().unwrap(),
    ]);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    assert!(
        stderr(&answer).contains(
            "Error: registry.targets[0].release_platform: must be one of ['darwin-arm64', \
             'linux-amd64'] and must be confirmed by host inventory"
        ),
        "got: {}",
        said(&answer)
    );

    // A platform the product does publish, declared for a machine that is not
    // it, is a valid document — and the refusal the validator defers to host
    // inventory for. So ask this machine: it contradicts the declaration in
    // the report's own verdict.
    let mut mistaken = policy.document();
    mistaken["targets"][0]["release_platform"] = json!(other_platform());
    let host = Host::new(&mistaken);
    let answer = host.stado(&[
        "registry",
        "validate",
        host.registry_path().to_str().unwrap(),
    ]);
    assert!(
        answer.status.success(),
        "a published platform must validate: {}",
        said(&answer)
    );
    let answer = host.stado(&["host", "inventory", TARGET, "--json"]);
    assert!(answer.status.success(), "{}", said(&answer));
    let inventory = report(&answer);
    assert_eq!(inventory["declared_release_platform"], other_platform());
    assert_eq!(
        inventory["release_platform"],
        platform(),
        "the inventory did not read this machine: {inventory}"
    );
    assert_eq!(inventory["release_platform_verdict"], "mismatched");
}
