//! A LAN connection path names the host, never the address its network
//! leased it. A router reassigns that address, and a path that still names the
//! old one answers "Host is down" for a host that is up.

use super::*;

fn fleet() -> tempfile::TempDir {
    seed(&json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "targets": [{
            "name": "journey-host",
            "kind": "local",
            "ssh": "operator@journey-preferred.example",
            "release_platform": "linux-amd64",
            "hostnames": ["journey-host.local"],
        }],
        "coordinators": [],
    }))
}

/// The host's own declared `.local` name is accepted as its LAN path: that
/// name is the same host, and it follows whatever address the router gives.
/// A second path naming that identity is still two routes to one identity.
#[test]
fn a_lan_path_by_the_hosts_own_name_is_accepted_once() {
    let directory = fleet();
    let storage = directory.path();

    let added = declare_path(
        storage,
        true,
        "journey-host",
        "lan",
        "operator@journey-host.local",
        None,
    );
    assert!(added.status.success(), "got: {}", stderr(&added));
    assert_eq!(document(&added)["changed"], true);
    let registry: Value =
        serde_json::from_slice(&std::fs::read(storage.join("registry.json")).unwrap()).unwrap();
    assert_eq!(
        registry["targets"][0]["ssh_fallbacks"],
        json!([{"name": "lan", "destination": "operator@journey-host.local"}])
    );

    let second = declare_path(
        storage,
        true,
        "journey-host",
        "lan-again",
        "operator@journey-host.local",
        None,
    );
    assert_eq!(second.status.code(), Some(1), "got: {}", stdout(&second));
    assert!(
        document(&second)["message"]
            .as_str()
            .unwrap()
            .contains("host identity 'journey-host.local' is already declared by"),
        "got: {}",
        stdout(&second)
    );
}

/// A path naming a private-network address is refused before the registry
/// moves, and the refusal says what to name instead.
#[test]
fn a_lan_path_by_a_leased_address_is_refused_and_changes_nothing() {
    let directory = fleet();
    let storage = directory.path();
    let before = std::fs::read(storage.join("registry.json")).unwrap();

    // RFC 5737 documentation space is not private, so the private example is
    // the one RFC 1918 reserves for exactly this kind of text.
    let refused = declare_path(
        storage,
        true,
        "journey-host",
        "lan",
        "operator@192.168.0.2",
        None,
    );
    assert_eq!(refused.status.code(), Some(1), "got: {}", stdout(&refused));
    let failure = document(&refused);
    assert_eq!(failure["error_code"], "refused");
    let message = failure["message"].as_str().unwrap();
    assert!(
        message.contains("names the lease-assigned address 192.168.0.2")
            && message.contains("name the host by its network name"),
        "got: {message}"
    );
    assert_eq!(
        std::fs::read(storage.join("registry.json")).unwrap(),
        before,
        "a refused path change must leave the document byte-identical"
    );
}
