//! `stado market peer` declares the Stado fleets this fleet lends to.
//!
//! One isolated deployment, its own store: no peer is declared at first; a
//! peer is declared with its object API origin, queue namespace and the
//! credential role whose `token` is its bearer, and `show` and `list` read
//! that declaration back from the store; declaring it a second time is
//! refused naming `edit`; `edit` changes only what it names and refuses a
//! call that names nothing; `remove` retires it, after which `show` is refused
//! naming the peers that are declared and `list` is empty again. An origin
//! with a path, a name with capitals, a provider-stado verb without `--peer`
//! and a `--peer` given to Vast.ai are each refused with the reason.
use crate::deployment::Deployment;
use serde_json::{json, Value};

const PEER: &str = "peer-journey";
const ORIGIN: &str = "https://objects.peer-journey.example";
const MOVED: &str = "https://objects-moved.peer-journey.example";

fn json_of(text: &str) -> Value {
    match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("the command did not print JSON ({error}): {text}"),
    }
}

/// Run `args`, which must be refused, and answer what it said on stderr.
fn refused(deployment: &mut Deployment, args: &[&str]) -> String {
    let (succeeded, stdout, stderr) = deployment.run(args, "");
    assert!(!succeeded, "stado {args:?} was not refused: {stdout}");
    stderr
}

#[test]
fn a_peer_fleet_is_declared_changed_and_retired() {
    let mut deployment = Deployment::start("peer-declaration");
    let empty = json_of(&deployment.cli(&["market", "peer", "list", "--json"]));
    assert_eq!(empty, json!([]), "a new fleet declares no peer");

    let declared = json_of(&deployment.cli(&[
        "market",
        "peer",
        "add",
        PEER,
        "--origin",
        ORIGIN,
        "--namespace",
        "peerqueue",
        "--credential-role",
        "peer-journey-bearer",
        "--json",
    ]));
    assert_eq!(declared["name"], json!(PEER), "{declared}");
    assert_eq!(declared["origin"], json!(ORIGIN), "{declared}");
    assert_eq!(declared["namespace"], json!("peerqueue"), "{declared}");
    assert_eq!(declared["credential_role"], json!("peer-journey-bearer"), "{declared}");
    assert_eq!(declared["offers"], json!([]), "{declared}");

    let shown = json_of(&deployment.cli(&["market", "peer", "show", PEER, "--json"]));
    assert_eq!(shown["origin"], json!(ORIGIN), "{shown}");
    assert_eq!(shown["declared_at"], declared["declared_at"], "{shown}");
    let listed = json_of(&deployment.cli(&["market", "peer", "list", "--json"]));
    assert_eq!(
        listed
            .as_array()
            .map(|peers| peers.iter().map(|peer| peer["name"].clone()).collect::<Vec<_>>()),
        Some(vec![json!(PEER)]),
        "{listed}"
    );

    let twice = refused(
        &mut deployment,
        &[
            "market",
            "peer",
            "add",
            PEER,
            "--origin",
            ORIGIN,
            "--namespace",
            "peerqueue",
            "--credential-role",
            "peer-journey-bearer",
        ],
    );
    assert!(twice.contains("already declared") && twice.contains("edit"), "{twice}");

    let changed = json_of(&deployment.cli(&[
        "market", "peer", "edit", PEER, "--origin", MOVED, "--json",
    ]));
    assert_eq!(changed["origin"], json!(MOVED), "{changed}");
    assert_eq!(changed["namespace"], json!("peerqueue"), "edit changed what it did not name: {changed}");
    assert_eq!(changed["declared_at"], declared["declared_at"], "{changed}");
    let nothing = refused(&mut deployment, &["market", "peer", "edit", PEER]);
    assert!(nothing.contains("names nothing to change"), "{nothing}");

    let with_path = refused(
        &mut deployment,
        &[
            "market",
            "peer",
            "add",
            "peer-journey-path",
            "--origin",
            "https://objects.peer-journey.example/queue",
            "--namespace",
            "peerqueue",
            "--credential-role",
            "peer-journey-bearer",
        ],
    );
    assert!(with_path.contains("bare origin"), "{with_path}");
    let capitals = refused(
        &mut deployment,
        &[
            "market",
            "peer",
            "add",
            "Peer-Journey",
            "--origin",
            ORIGIN,
            "--namespace",
            "peerqueue",
            "--credential-role",
            "peer-journey-bearer",
        ],
    );
    assert!(capitals.contains("lowercase"), "{capitals}");
    let no_peer = refused(&mut deployment, &["market", "unlist", "--provider", "stado"]);
    assert!(no_peer.contains("--peer NAME"), "{no_peer}");
    let vast_peer = refused(
        &mut deployment,
        &["market", "unlist", "--provider", "vast", "--peer", PEER],
    );
    assert!(vast_peer.contains("--provider stado"), "{vast_peer}");

    let removed = json_of(&deployment.cli(&["market", "peer", "remove", PEER, "--json"]));
    assert_eq!(removed, json!({"peer": PEER, "removed": true}));
    let gone = refused(&mut deployment, &["market", "peer", "show", PEER]);
    assert!(
        gone.contains("no peer fleet named") && gone.contains("no peer is declared"),
        "{gone}"
    );
    let after = json_of(&deployment.cli(&["market", "peer", "list", "--json"]));
    assert_eq!(after, json!([]), "a retired peer is still listed: {after}");
    deployment.pass();
}
