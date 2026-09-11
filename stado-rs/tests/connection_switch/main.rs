//! Moving a host off tailscale onto a network drawn at random from the
//! product's own declared list, through the real `stado` binary.
//!
//! The fleet reaches most of its hosts over one network, and the question this
//! journey answers is whether a host can be moved onto another one by editing
//! nothing but the registry: no per-network remote-execution policy, no second
//! credential, no code change. The network is not written here — it is read
//! out of `known_providers`, the vocabulary
//! `stado-rs/data/fleet/connections.json` declares and
//! `stado registry host path list` publishes, and one of them is drawn with an
//! operating-system seeded hasher, so a run exercises whichever network the
//! draw lands on and prints it as its evidence.
//!
//! Nothing leaves this machine. The registry is an isolated store under this
//! checkout's build directory, every route is declared through the product's
//! own command, and every destination is a name inside `.invalid`, which
//! RFC 2606 reserves so it can never resolve.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};
use stado::targets::{PRIMARY_SSH_CONNECTION, REGISTRY_SCHEMA_VERSION};

/// The fixture host. A name, not one of the operator's machines: this journey
/// rewrites the host's routes, so it must own the target it edits.
const HOST: &str = "connection-switch-journey";

/// The network the host starts on, which this journey moves it off.
const START_NETWORK: &str = "tailscale";

/// RFC 2606 reserves `.invalid`, so no destination written here resolves.
const UNROUTABLE_SUFFIX: &str = ".invalid";

fn runs_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/connection-switch-runs");
    std::fs::create_dir_all(&root).expect("create this journey's run root");
    root
}

fn stado(storage: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .output()
        .expect("stado binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn document(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!(
            "expected one JSON document, got {error}\nstdout: {}\nstderr: {}",
            stdout(output),
            stderr(output)
        )
    })
}

/// An isolated registry holding one host, reached at the destination its own
/// `ssh` entry names. Every named route this journey works with is then
/// declared through the product's own command.
fn seed(destination: &str) -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("connection-switch-")
        .tempdir_in(runs_root())
        .expect("create the isolated store");
    let registry = json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "targets": [{
            "name": HOST,
            "kind": "local",
            "hostname": format!("{HOST}{UNROUTABLE_SUFFIX}"),
            "ssh": destination,
            "release_platform": "darwin-arm64",
        }],
    });
    std::fs::write(
        directory.path().join("registry.json"),
        format!("{}\n", serde_json::to_string_pretty(&registry).unwrap()),
    )
    .expect("write the isolated registry");
    directory
}

fn path_list(storage: &Path) -> Value {
    let listing = stado(
        storage,
        &["registry", "host", "path", "list", HOST, "--json"],
    );
    assert!(
        listing.status.success(),
        "the listing failed: {}",
        stderr(&listing)
    );
    document(&listing)
}

fn declared_networks(listing: &Value) -> Vec<String> {
    listing["known_providers"]
        .as_array()
        .expect("the listing publishes the networks the product declares")
        .iter()
        .map(|provider| {
            provider["name"]
                .as_str()
                .expect("a declared network has a name")
                .to_string()
        })
        .collect()
}

/// Draw one of the declared networks with the operating system's own
/// randomness. `RandomState` is seeded per process by the platform, so the
/// draw is a real draw rather than a fixed choice dressed up as one.
fn draw(candidates: &[String]) -> &str {
    assert!(
        !candidates.is_empty(),
        "the product declares no network to move onto besides {START_NETWORK} and {PRIMARY_SSH_CONNECTION}"
    );
    let mut hasher = RandomState::new().build_hasher();
    hasher.write(START_NETWORK.as_bytes());
    let span = u64::try_from(candidates.len()).expect("the declared list fits in a draw");
    let index = usize::try_from(hasher.finish() % span).expect("an index inside the declared list");
    candidates[index].as_str()
}

fn set_path(storage: &Path, network: &str, destination: &str, rank: Option<&str>) -> Value {
    let mut args = vec![
        "registry",
        "host",
        "path",
        "set",
        HOST,
        network,
        "--ssh",
        destination,
    ];
    if let Some(rank) = rank {
        args.extend(["--priority", rank]);
    }
    args.push("--json");
    let output = stado(storage, &args);
    assert!(
        output.status.success(),
        "setting {network} failed: {}",
        stderr(&output)
    );
    document(&output)
}

/// The journey: a host reached over one network is moved onto another network
/// the product itself names, and the move is read back out of the registry.
///
/// The order is the product's own: two routes may not name one host identity,
/// so the drawn network is declared beside the starting one, then the roles
/// are swapped - the preferred entry takes the drawn destination once no named
/// route holds it, and the address the host came off stays declared under its
/// own network name.
#[test]
fn a_host_moves_off_tailscale_onto_a_declared_network_drawn_at_random() {
    let start_destination = format!("operator@{HOST}-{START_NETWORK}{UNROUTABLE_SUFFIX}");
    let store = seed(&start_destination);
    let storage = store.path();

    let before = path_list(storage);
    let networks = declared_networks(&before);
    assert!(
        networks.contains(&START_NETWORK.to_string()),
        "the product does not declare the network this host starts on: {networks:?}"
    );
    assert_eq!(
        before["connections"].as_array().map(Vec::len),
        Some(1),
        "the host starts on exactly one route: {before}"
    );
    let candidates = networks
        .iter()
        .filter(|name| name.as_str() != START_NETWORK && name.as_str() != PRIMARY_SSH_CONNECTION)
        .cloned()
        .collect::<Vec<String>>();
    let drawn = draw(&candidates).to_string();
    let destination = format!("operator@{HOST}-{drawn}{UNROUTABLE_SUFFIX}");

    // The drawn network is declared beside the one in use, so the host can be
    // reached over it before anything is moved.
    let added = set_path(storage, &drawn, &destination, Some("1"));
    assert_eq!(added["changed"], json!(true), "{added}");
    let again = set_path(storage, &drawn, &destination, None);
    assert_eq!(
        again["changed"],
        json!(false),
        "declaring the same route twice moved the registry: {again}"
    );

    // The move itself: the named route gives the identity up, the preferred
    // entry takes it, and the address the host came off is declared under the
    // network it belongs to.
    let released = stado(
        storage,
        &["registry", "host", "path", "remove", HOST, &drawn, "--json"],
    );
    assert!(
        released.status.success(),
        "releasing {drawn} failed: {}",
        stderr(&released)
    );
    let promoted = set_path(storage, PRIMARY_SSH_CONNECTION, &destination, None);
    assert_eq!(promoted["changed"], json!(true), "{promoted}");
    let demoted = set_path(storage, START_NETWORK, &start_destination, Some("1"));
    assert_eq!(demoted["changed"], json!(true), "{demoted}");

    // The proof is the product's own read and the document on disk, not the
    // sentences the mutations printed.
    let after = path_list(storage);
    let connections = after["connections"]
        .as_array()
        .expect("the listing carries the host's routes")
        .clone();
    let preferred = connections
        .first()
        .expect("a host with a declared ssh entry has a preferred route");
    assert_eq!(preferred["name"], json!(PRIMARY_SSH_CONNECTION), "{after}");
    assert_eq!(preferred["destination"], json!(destination), "{after}");
    assert_eq!(preferred["preferred"], json!(true), "{after}");
    assert!(
        connections
            .iter()
            .any(|route| route["name"] == json!(START_NETWORK)
                && route["destination"] == json!(start_destination)
                && route["preferred"] == json!(false)),
        "the network the host came off is not declared behind the new one: {after}"
    );

    let stored = std::fs::read_to_string(storage.join("registry.json"))
        .expect("the isolated registry document is on disk");
    assert!(
        stored.contains(&destination) && stored.contains(&start_destination),
        "the registry does not carry both declared destinations: {stored}"
    );

    // One identity, one route: asking for the drawn network again, at the
    // address the preferred entry now holds, is refused and changes nothing.
    let duplicate = stado(
        storage,
        &[
            "registry",
            "host",
            "path",
            "set",
            HOST,
            &drawn,
            "--ssh",
            &destination,
            "--json",
        ],
    );
    assert!(
        !duplicate.status.success(),
        "a second route took the identity the preferred entry holds: {}",
        stdout(&duplicate)
    );
    let complaint = stderr(&duplicate);
    // The identity in the sentence is the host the destination names, without
    // the login in front of it: one machine may be reached as two users and is
    // still one machine.
    let identity = destination
        .split_once('@')
        .map(|(_, host)| host)
        .expect("the fixture destination carries a login");
    assert!(
        complaint.contains(&format!("host identity '{identity}' is already declared")),
        "the refusal does not name the identity: {complaint}"
    );
    assert_eq!(
        std::fs::read_to_string(storage.join("registry.json")).expect("the document is readable"),
        stored,
        "the refused write moved the registry"
    );

    println!(
        "moved {HOST} off {START_NETWORK} onto {drawn} at {destination}; the product declares {networks:?}"
    );
}
