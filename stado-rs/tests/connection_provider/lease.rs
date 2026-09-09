//! The real journey: a registered host whose preferred path cannot resolve,
//! reached over its second declared route, with the receipt naming that route
//! and carrying the machine's own output.
//!
//! The host is not an operator's choice in an environment variable. It is a
//! disposable target this case leases with `stado scratch` on whichever
//! registered host the fleet says a lease may be taken on, so the case runs by
//! default, writes nothing anybody keeps, and destroys what it took. The lease
//! emits its own registry document; that document — never the canonical
//! registry — is what the declared routes are added to.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use crate::{
    declare_path, document, exec_uptime, stderr, stdout, SECOND_PATH, UNROUTABLE_SUFFIX,
};

/// The lifetime the lease is taken for. Long enough for one read over ssh and
/// short enough that a crashed case leaves nothing the reaper will not sweep;
/// this constants/config/tuning value is a declared name copied from the
/// scratch area's own live runs and tunes nothing.
const LEASE_TTL: &str = "15m";

/// The built binary with the operator's environment intact: leasing resolves
/// its host and its brokered key through the fleet configuration, and this
/// journey deliberately does not fake either.
fn fleet(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .output()
        .expect("stado binary runs")
}

/// The fleet's own answer to "where may a lease be taken", preferring a host
/// that is not this machine so the command really crosses ssh.
fn leasable_host() -> (String, String) {
    let arguments = ["scratch", "hosts", "--json"];
    let listed = fleet(&arguments);
    assert!(
        listed.status.success(),
        "the fleet could not say which hosts are leasable, so this run is blocked: {}{}",
        stdout(&listed),
        stderr(&listed)
    );
    let report = document(&listed);
    let hosts = report["hosts"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the hosts report carries no hosts array: {report}"));
    let local = Command::new("hostname")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_lowercase())
        .unwrap_or_default();
    let mut eligible: Vec<(String, String)> = hosts
        .iter()
        .filter(|row| row["eligible"].as_bool().unwrap_or_default())
        .map(|row| {
            (
                row["target"].as_str().unwrap_or_default().to_string(),
                row["profile"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        !eligible.is_empty(),
        "no registry target is leasable, so this run is blocked rather than passed: {report}"
    );
    if let Some(index) = eligible
        .iter()
        .position(|(target, _)| !local.starts_with(target) && !target.starts_with(&local))
    {
        return eligible.swap_remove(index);
    }
    eligible.swap_remove(0)
}

/// One lease, and everything the caller needs to drive it.
struct Lease {
    host: String,
    name: String,
    storage_root: String,
    destination: String,
}

impl Lease {
    fn take() -> Self {
        let (host, profile) = leasable_host();
        let arguments = [
            "scratch",
            "create",
            "--host",
            &host,
            "--profile",
            &profile,
            "--ttl",
            LEASE_TTL,
            "--json",
        ];
        let created = fleet(&arguments);
        assert!(
            created.status.success(),
            "no lease could be taken on {host}, so this run is blocked: {}{}",
            stdout(&created),
            stderr(&created)
        );
        let report = document(&created);
        assert_eq!(report["status"], "leased", "{report}");
        let name = report["name"]
            .as_str()
            .expect("the lease report names the lease")
            .to_string();
        let storage_root = report["storage_root"]
            .as_str()
            .expect("the lease emits a storage root")
            .to_string();
        let registry_path = report["registry_path"]
            .as_str()
            .expect("the lease emits a registry document");
        let declared: Value = serde_json::from_str(
            &std::fs::read_to_string(registry_path).expect("the emitted registry is readable"),
        )
        .expect("the emitted registry is JSON");
        let destination = declared["targets"][0]["ssh"]
            .as_str()
            .unwrap_or_else(|| panic!("the leased target declares no ssh destination: {declared}"))
            .to_string();
        assert!(
            destination.starts_with(&name),
            "the leased target must log in as the lease: {declared}"
        );
        Self {
            host,
            name,
            storage_root,
            destination,
        }
    }

    fn storage(&self) -> &Path {
        Path::new(&self.storage_root)
    }

    /// Destroy the lease and read the host's own account of what is left.
    fn destroy(&self) -> Value {
        let arguments = [
            "scratch",
            "destroy",
            &self.name,
            "--host",
            &self.host,
            "--json",
        ];
        let destroyed = fleet(&arguments);
        assert!(
            destroyed.status.success(),
            "the lease {} was not destroyed: {}{}",
            self.name,
            stdout(&destroyed),
            stderr(&destroyed)
        );
        document(&destroyed)
    }
}

/// A preferred route that cannot resolve is handed over to the second declared
/// route, the receipt names the route that answered rather than the one that
/// was declared first, and it carries the leased machine's own output.
#[test]
fn a_dead_preferred_path_hands_over_and_the_receipt_names_the_route() {
    let lease = Lease::take();
    let storage = lease.storage();
    let account = lease
        .destination
        .split_once('@')
        .expect("an ssh destination")
        .0
        .to_string();

    // Preferred first, so the real destination is never declared twice.
    let unroutable = format!("{account}@{}{UNROUTABLE_SUFFIX}", lease.name);
    let preferred = declare_path(storage, false, &lease.name, "primary", &unroutable, None);
    assert!(preferred.status.success(), "{}", stderr(&preferred));
    let second = declare_path(
        storage,
        false,
        &lease.name,
        SECOND_PATH,
        &lease.destination,
        Some("1"),
    );
    assert!(second.status.success(), "{}", stderr(&second));

    let output = exec_uptime(storage, false, &lease.name);
    let receipt = document(&output);
    let destroyed = lease.destroy();

    assert_eq!(receipt["status"], "ok", "stderr: {}", stderr(&output));
    assert_eq!(receipt["ssh"], unroutable.as_str());
    assert_eq!(receipt["used_connection"]["kind"], "ssh");
    assert_eq!(
        receipt["used_connection"]["name"], SECOND_PATH,
        "the receipt must name the route that answered, not a declaration"
    );
    assert_eq!(
        receipt["used_connection"]["destination"],
        lease.destination.as_str()
    );
    assert!(
        receipt["stdout"]
            .as_str()
            .expect("the receipt carries the host's own stdout")
            .contains("load average"),
        "the leased machine's own uptime output is missing: {}",
        receipt["stdout"]
    );
    assert_eq!(output.status.code(), Some(0));

    assert_eq!(destroyed["status"], "destroyed", "{destroyed}");
    assert_eq!(destroyed["account"], "absent", "{destroyed}");
    assert_eq!(destroyed["record"], "absent", "{destroyed}");
    assert_eq!(
        fleet(&["scratch", "list", "--host", &lease.host, "--json"])
            .status
            .success(),
        true
    );
    let held = document(&fleet(&[
        "scratch",
        "list",
        "--host",
        &lease.host,
        "--json",
    ]));
    assert_eq!(
        held["leases"]
            .as_array()
            .map(|rows| rows
                .iter()
                .any(|row| row["name"].as_str() == Some(lease.name.as_str())))
            .unwrap_or_default(),
        false,
        "the host still reports the lease this case took: {held}"
    );
    assert_eq!(json!(Path::new(&lease.storage_root).exists()), json!(false));
}
