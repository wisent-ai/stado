//! Which declared connection path carried a host operation, through the real
//! `stado` binary.
//!
//! Every test drives `CARGO_BIN_EXE_stado` against an isolated registry
//! (`WC_STORAGE_BACKEND=local` + `WC_LOCAL_STORAGE_PATH=<TempDir>`) with `HOME`
//! inside that same tempdir, and each route is added through `registry host
//! path set`, so the document under test is the one the product wrote and the
//! canonical registry — and the operator's own `~/.stado` cache — are
//! untouched.
//!
//! What is defended: a receipt used to publish only the declared routes and
//! never which one carried the command, so a host whose preferred route was
//! dead read like a healthy one. `used_connection` is that missing fact.

mod fixture;
#[path = "../support/owned_home.rs"]
mod owned_home;

use std::path::Path;
use std::process::Output;

use serde_json::{json, Value};
use stado::targets::REGISTRY_SCHEMA_VERSION;

use fixture::{document, pull_canonical, seed, stado, stderr, this_hostname};

/// The host whose real journey is exercised. Supplied explicitly: a test that
/// chose a fleet host itself would send ssh traffic at whatever the
/// developer's registry happens to hold.
const HOST_VARIABLE: &str = "STADO_CONNECTION_PROVIDER_HOST";

/// RFC 2606 reserves `.invalid`, so the preferred path cannot resolve and the
/// second declared route is the only one left. The host is not touched.
const UNROUTABLE_SUFFIX: &str = ".invalid";

/// The second declared route these tests add.
const SECOND_PATH: &str = "journey-alternate";

/// One `registry host path set`, in the shape all four call sites need: a
/// route name, its destination, an optional rank, and the typed receipt.
fn declare_path(
    storage: &Path,
    isolated_config: bool,
    host: &str,
    path: &str,
    destination: &str,
    rank: Option<&str>,
) -> Output {
    let mut args = vec!["registry", "host", "path", "set", host, path, "--ssh"];
    args.push(destination);
    if let Some(rank) = rank {
        args.extend(["--priority", rank]);
    }
    args.push("--json");
    stado(storage, isolated_config, &args)
}

/// The one approved read these tests run on a host.
fn exec_uptime(storage: &Path, isolated_config: bool, host: &str) -> Output {
    let args = ["host", "exec", "--json", host, "--", "uptime"];
    stado(storage, isolated_config, &args)
}

/// A target on this machine runs its command through the local channel, and
/// the receipt says so instead of naming the preferred declared path -- which
/// would be an invention, because no declared path was used.
#[test]
fn a_command_on_this_machine_reports_the_local_channel() {
    let hostname = this_hostname();
    assert!(!hostname.is_empty(), "this machine has no hostname");
    let directory = seed(&json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "targets": [{
            "name": "journey-local",
            "kind": "local",
            "release_platform": "darwin-arm64",
            "hostnames": [hostname],
        }],
        "coordinators": [],
    }));

    let output = exec_uptime(directory.path(), true, "journey-local");
    let receipt = document(&output);
    assert_eq!(receipt["schema"], "stado.host-exec-receipt.v1");
    assert_eq!(receipt["status"], "ok", "stderr: {}", stderr(&output));
    assert_eq!(receipt["used_connection"]["kind"], "local");
    assert_eq!(receipt["used_connection"]["name"], "local");
    // A local channel has no ssh destination to report.
    assert_eq!(receipt["used_connection"]["destination"], Value::Null);
    assert_eq!(receipt["ssh"], Value::Null);
    assert!(
        receipt["stdout"].as_str().unwrap().contains("load average"),
        "the host's own uptime output is missing: {}",
        receipt["stdout"]
    );
    assert_eq!(output.status.code(), Some(0));
}

/// Two paths may not name one host identity. The refusal states its own code,
/// answers a `--json` caller with a document, and leaves the registry exactly
/// as it was -- this validation refusal used to arrive as `unknown`, under
/// "your request or credentials", which sends an operator to check a
/// credential for a change the registry rejected on its own rules.
#[test]
fn a_duplicate_route_is_a_typed_refusal_that_changes_nothing() {
    let directory = seed(&json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "targets": [{
            "name": "journey-host",
            "kind": "local",
            "ssh": "operator@journey-preferred.example",
            "release_platform": "linux-amd64",
            "hostnames": ["journey-host.example"],
        }],
        "coordinators": [],
    }));
    let storage = directory.path();

    let added = declare_path(
        storage,
        true,
        "journey-host",
        SECOND_PATH,
        "operator@journey-host.local",
        Some("1"),
    );
    assert!(added.status.success(), "got: {}", stderr(&added));
    assert_eq!(document(&added)["changed"], true);
    let before = std::fs::read(storage.join("registry.json")).unwrap();

    let identity = "operator@journey-preferred.example";
    let refused = declare_path(storage, true, "journey-host", "third", identity, None);
    assert_eq!(refused.status.code(), Some(1));
    let failure = document(&refused);
    assert_eq!(failure["status"], "error");
    assert_eq!(failure["error_code"], "refused");
    assert_eq!(failure["retryable"], false);
    assert_eq!(failure["failure_point"], "cli.registry.host.path.set");
    assert_eq!(
        failure["summary"],
        "an explicit policy refused this command"
    );
    assert!(
        failure["message"]
            .as_str()
            .unwrap()
            .contains("host identity 'journey-preferred.example' is already declared"),
        "the refusal names the duplicated identity: {}",
        failure["message"]
    );
    assert_eq!(
        std::fs::read(storage.join("registry.json")).unwrap(),
        before,
        "a refused path change must leave the document byte-identical"
    );

    // The primary is preferred by definition, so it takes no rank.
    let misuse = stado(
        storage,
        true,
        &[
            "registry",
            "host",
            "path",
            "set",
            "journey-host",
            "primary",
            "--ssh",
            "operator@journey-preferred.example",
            "--priority",
            "1",
        ],
    );
    assert_eq!(misuse.status.code(), Some(1));
    assert!(
        stderr(&misuse)
            .contains("the primary path is always preferred and does not take --priority"),
        "got: {}",
        stderr(&misuse)
    );
}

/// The real journey: a registered fleet host whose preferred path cannot
/// resolve, reached over its second declared route, with the receipt naming
/// that route and carrying the host's own output.
///
/// Ignored by default because it needs a registered host and its brokered
/// key. It writes nothing on the host and nothing to the canonical registry:
/// the fixture lives in a temp store, and `uptime` is a read.
#[test]
#[ignore = "needs STADO_CONNECTION_PROVIDER_HOST, a registered fleet host reachable over ssh"]
fn a_dead_preferred_path_hands_over_and_the_receipt_names_the_route() {
    let host = std::env::var(HOST_VARIABLE)
        .unwrap_or_else(|_| panic!("{HOST_VARIABLE} must name a registered fleet host"));

    // The real destination comes from the canonical registry through the
    // product itself, never from a literal in this file.
    let pulled = pull_canonical();
    let canonical: Value = serde_json::from_slice(&pulled.stdout)
        .unwrap_or_else(|_| panic!("registry pull: {}", stderr(&pulled)));
    let target = canonical["targets"]
        .as_array()
        .expect("registry.targets is an array")
        .iter()
        .find(|entry| entry["name"] == host.as_str())
        .unwrap_or_else(|| panic!("{host} is not a registry target"))
        .clone();
    let reachable = target["ssh"]
        .as_str()
        .unwrap_or_else(|| panic!("{host} declares no ssh destination"))
        .to_string();
    let (account, _) = reachable.split_once('@').expect("an ssh destination");

    let directory = seed(&json!({
        "schema_version": canonical["schema_version"],
        "targets": [target],
        "coordinators": [],
    }));
    let storage = directory.path();

    // Preferred path first, so the real destination is never declared twice.
    let unroutable = format!("{account}@{host}{UNROUTABLE_SUFFIX}");
    let preferred = declare_path(storage, false, &host, "primary", &unroutable, None);
    assert!(preferred.status.success(), "{}", stderr(&preferred));
    let second = declare_path(storage, false, &host, SECOND_PATH, &reachable, Some("1"));
    assert!(second.status.success(), "{}", stderr(&second));

    let output = exec_uptime(storage, false, &host);
    let receipt = document(&output);
    assert_eq!(receipt["status"], "ok", "stderr: {}", stderr(&output));
    assert_eq!(receipt["ssh"], unroutable.as_str());
    assert_eq!(receipt["used_connection"]["kind"], "ssh");
    assert_eq!(
        receipt["used_connection"]["name"], SECOND_PATH,
        "the receipt must name the route that answered, not a declaration"
    );
    assert_eq!(
        receipt["used_connection"]["destination"],
        reachable.as_str()
    );
    assert!(
        receipt["stdout"].as_str().unwrap().contains("load average"),
        "the host's own uptime output is missing: {}",
        receipt["stdout"]
    );
    assert_eq!(output.status.code(), Some(0));
}
