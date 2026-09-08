//! The disposable target the leased routing cases run against.
//!
//! A lease is a real account on a real registry host, minted by `stado scratch
//! create` and declared in a registry document of its own. Everything this
//! module does is the product's: it asks the fleet which hosts are leasable,
//! takes the account, seeds a service directory into the emitted registry,
//! declares the service through `stado service declare`, reads the leased
//! account's forwards directory back through `stado host inventory`, and gives
//! the account back. A case that cannot get a lease fails with the fleet's own
//! refusal; it never falls back to a tempdir, because a tempdir would answer a
//! different question.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::fleet::{json_stdout, said};

/// The service the leased cases declare. Nothing listens at its endpoint and
/// nothing dials it: what travels is the declaration, out of the registry and
/// into a marker in another account's home.
pub const SERVICE: &str = "route-leased-service";
/// The loopback address the leased host is told to call.
pub const ENDPOINT: &str = "http://127.0.0.1:48232";
/// Long enough for one case, short enough that a leaked lease is swept.
const LEASE_LIFETIME: &str = "15m";

/// One host-mutating story at a time: `create` runs the host's expiry sweep
/// before it mints an account, so two interleaved cases would have one lease
/// swept by the other's create and blame the reaper for working.
static HOST: Mutex<()> = Mutex::new(());

pub fn with_host_turn<T>(story: impl FnOnce() -> T) -> T {
    let turn = HOST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let answer = story();
    drop(turn);
    answer
}

/// The built binary with the operator's environment intact: the fleet
/// configuration a lease needs to resolve its host is the operator's, and this
/// area deliberately does not fake it.
fn stado(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("stado {arguments:?} did not start: {error}"))
}

pub fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

pub fn rows(value: &Value, key: &str) -> Vec<Value> {
    value[key].as_array().cloned().unwrap_or_default()
}

/// One host a lease may be taken on, as the fleet describes it.
pub struct Leasable {
    pub target: String,
    pub profile: String,
}

/// Ask the product where a lease may be taken. A run that cannot get a host is
/// blocked, never passed, and never quietly moved into a tempdir.
pub fn leasable_host() -> Leasable {
    let output = stado(&["scratch", "hosts", "--json"]);
    assert!(
        output.status.success(),
        "the fleet could not say which hosts are leasable, so this run is blocked:\n{}",
        said(&output)
    );
    let report = json_stdout(&output);
    let local = hostname();
    let mut eligible: Vec<Leasable> = rows(&report, "hosts")
        .iter()
        .filter(|row| row["eligible"].as_bool().unwrap_or_default())
        .map(|row| Leasable {
            target: text(row, "target"),
            profile: text(row, "profile"),
        })
        .collect();
    assert!(
        !eligible.is_empty(),
        "no registry target is leasable, so this run is blocked rather than passed: {report}"
    );
    // A machine other than this one, so the marker really crosses the channel
    // into another account instead of being written beside the operator.
    if let Some(index) = eligible
        .iter()
        .position(|row| !local.starts_with(&row.target) && !row.target.starts_with(&local))
    {
        return eligible.swap_remove(index);
    }
    eligible.into_iter().next().expect("proved non-empty above")
}

fn hostname() -> String {
    Command::new("hostname")
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_lowercase()
        })
        .unwrap_or_default()
}

/// A leased account and the isolated registry that declares it.
pub struct Lease {
    pub name: String,
    pub target: String,
    pub home: String,
    pub root: String,
    pub registry: String,
    destroyed: bool,
}

impl Lease {
    /// The binary pointed at the emitted registry and at no operator config.
    pub fn stado(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.root)
            .env("STADO_CONFIG", Path::new(&self.root).join("no-such-config"))
            .output()
            .unwrap_or_else(|error| panic!("stado {arguments:?} did not start: {error}"))
    }

    /// Where the leased account's marker must live, derived from the home the
    /// create report named rather than from the answer `route` gives.
    pub fn marker(&self) -> String {
        format!("{}/.stado/forwards/{SERVICE}.local", self.home)
    }

    /// What the leased machine says about that account's forwards directory,
    /// read through a command that shares no code with `route`.
    pub fn forwards_state(&self) -> String {
        let output = self.stado(&["host", "inventory", self.name.as_str(), "--json"]);
        assert!(
            output.status.success(),
            "the leased machine did not answer an inventory read:\n{}",
            said(&output)
        );
        text(&json_stdout(&output), "forwards_dir_state")
    }

    /// Give the account back, and hold the destroy to the three absences it
    /// reads off the host after deleting: no account, no home, no record.
    pub fn destroy(&mut self) {
        let (name, target) = (self.name.clone(), self.target.clone());
        self.destroyed = true;
        let output = stado(&["scratch", "destroy", &name, "--host", &target, "--json"]);
        assert!(
            output.status.success(),
            "the lease outlived its case:\n{}",
            said(&output)
        );
        let report = json_stdout(&output);
        assert_eq!(report["status"], "destroyed");
        assert_eq!(report["account"], "absent");
        assert_eq!(report["home"], "absent");
        assert_eq!(report["record"], "absent");
        assert_eq!(report["storage_root"], "removed");

        let listed = stado(&["scratch", "list", "--host", &target, "--json"]);
        assert!(listed.status.success(), "{}", said(&listed));
        let held = json_stdout(&listed);
        assert!(
            rows(&held, "leases")
                .iter()
                .all(|row| row["name"].as_str() != Some(name.as_str())),
            "{target} still holds {name} after the destroy said it did not: {held}"
        );
    }
}

/// A case that panics still gives the account back. No assertion lives here: a
/// panicking drop during unwind aborts the run and hides the real failure.
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.destroyed {
            let _ = Command::new(env!("CARGO_BIN_EXE_stado"))
                .args(["scratch", "destroy", &self.name, "--host", &self.target])
                .output();
        }
    }
}

pub fn lease_on(host: &Leasable) -> Lease {
    let output = stado(&[
        "scratch",
        "create",
        "--host",
        &host.target,
        "--profile",
        &host.profile,
        "--ttl",
        LEASE_LIFETIME,
        "--json",
    ]);
    assert!(
        output.status.success(),
        "no lease could be taken on {}, so this case fails with the fleet's own refusal:\n{}",
        host.target,
        said(&output)
    );
    let report = json_stdout(&output);
    assert_eq!(report["status"], "leased");
    assert_eq!(report["account"], "created");
    let lease = Lease {
        name: text(&report, "name"),
        target: host.target.clone(),
        home: text(&report, "home_path"),
        root: text(&report, "storage_root"),
        registry: text(&report, "registry_path"),
        destroyed: false,
    };
    assert!(
        !lease.name.is_empty() && !lease.home.is_empty() && !lease.registry.is_empty(),
        "the lease report is missing coordinates a route case needs: {report}"
    );
    lease
}

/// Give the emitted registry a service directory of its own, then declare the
/// service into it through the product's own writer.
pub fn declare_on(lease: &Lease) {
    let emitted = std::fs::read_to_string(&lease.registry).expect("the emitted registry reads");
    let mut document: Value = serde_json::from_str(&emitted).expect("the emitted registry is JSON");
    let leased = lease.name.as_str();
    document["service_directory"] = json!({
        "authority": {"target": leased, "command": env!("CARGO_BIN_EXE_stado")},
        "generation": crate::FIRST_GENERATION,
        "services": {},
    });
    let seeded = serde_json::to_string_pretty(&document).expect("the seeded registry serializes");
    std::fs::write(&lease.registry, format!("{seeded}\n")).expect("the emitted registry is ours");

    let file = Path::new(&lease.root).join("route-leased.declaration.json");
    let declaration = json!({
        "name": SERVICE,
        "host": leased,
        "source": {
            "artifact": format!("{SERVICE}/1.0.0/{SERVICE}.tar.gz"),
            "sha256": "0".repeat(crate::SHA256_HEX_LEN),
        },
        "run": {"program": "/usr/bin/true", "args": ["--serve"]},
        "consumers": {"stado-route-tests": {"capabilities": ["read"]}},
        "endpoints": {leased: {"url": ENDPOINT}},
    });
    let body = serde_json::to_string_pretty(&declaration).expect("the declaration serializes");
    std::fs::write(&file, body).expect("the lease store root is writable");
    let path = file.to_str().expect("the declaration path is text");
    let output = lease.stado(&["service", "declare", "--file", path, "--json"]);
    assert!(
        output.status.success(),
        "declaring the route on the leased target failed:\n{}",
        said(&output)
    );
}
