//! What the stories share: the binary, the fleet's own host list, and the lock
//! that keeps two host-mutating stories out of each other's way.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::{LazyLock, Mutex, MutexGuard};

use serde_json::Value;

use crate::owned_home::{copy_ssh_identity, copy_stado_config};

/// One host at a time. The stories that create and destroy leases run in one
/// process, and `create` reaps the host's expired leases before taking a new
/// one — so an unsynchronized expiry story would have its lease swept by the
/// lifecycle story's create, and blame the reaper for working.
static HOST: Mutex<()> = Mutex::new(());

/// Serialize the stories that change a host.
pub fn host_turn() -> MutexGuard<'static, ()> {
    HOST.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The home this area owns, shared by every story in the process.
///
/// The fleet configuration a lease needs to resolve its host stays the
/// operator's, and the ssh identity the host answers to stays the operator's
/// too — but both are copied in rather than reached for, because the product
/// records its last-known-good registry copy and the lease's own storage root
/// under `HOME`, and a spawn that inherited the operator's home would write
/// the operator's `~/.stado`. Every story reads its storage root out of the
/// product's report, so an owned home moves that state into this directory and
/// nothing else changes.
static HOME: LazyLock<tempfile::TempDir> = LazyLock::new(|| {
    let owned = tempfile::tempdir().expect("a home this area owns");
    copy_ssh_identity(owned.path());
    copy_stado_config(owned.path());
    owned
});

pub fn home() -> &'static Path {
    HOME.path()
}

/// The built binary, with the caller's fleet configuration intact and `HOME`
/// on the directory this area owns.
pub fn stado() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command.env("HOME", home());
    command
}

/// Run one command and hand back everything it said.
pub fn run(arguments: &[&str]) -> Output {
    stado()
        .args(arguments)
        .output()
        .unwrap_or_else(|exc| panic!("stado {arguments:?} did not start: {exc}"))
}

/// stdout as text.
pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// stderr as text.
pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// The one JSON document a `--json` command printed.
pub fn document(output: &Output, arguments: &[&str]) -> Value {
    assert!(
        output.status.success(),
        "stado {arguments:?} failed: {}{}",
        stdout(output),
        stderr(output)
    );
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|exc| panic!("stado {arguments:?} printed no JSON document: {exc}"))
}

/// One host a lease may be taken on, as the fleet describes it.
pub struct Leasable {
    pub target: String,
    pub profile: String,
    pub release_platform: String,
}

/// The fleet's own answer to "where can this run lease". Fails loudly, with the
/// report, when the answer is nowhere: a test that cannot get a host has not
/// passed.
pub fn leasable_host() -> Leasable {
    let arguments = ["scratch", "hosts", "--json"];
    let output = run(&arguments);
    assert!(
        output.status.success(),
        "the fleet could not say which hosts are leasable, so this run is blocked: {}{}",
        stdout(&output),
        stderr(&output)
    );
    let report = document(&output, &arguments);
    let hosts = report["hosts"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the hosts report carries no hosts array: {report}"));
    let local = hostname();
    let mut candidates: Vec<Leasable> = hosts
        .iter()
        .filter(|row| row["eligible"].as_bool().unwrap_or_default())
        .map(|row| Leasable {
            target: row["target"].as_str().unwrap_or_default().to_string(),
            profile: row["profile"].as_str().unwrap_or_default().to_string(),
            release_platform: row["release_platform"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        })
        .collect();
    assert!(
        !candidates.is_empty(),
        "no registry target is leasable, so this run is blocked rather than passed: {report}"
    );
    // A remote host first: leasing on the machine running the tests would create
    // the account beside the operator's own login instead of over the channel,
    // which is a different path from the one every other caller takes.
    if let Some(index) = candidates
        .iter()
        .position(|row| !local.starts_with(&row.target) && !row.target.starts_with(&local))
    {
        return candidates.swap_remove(index);
    }
    candidates.swap_remove(0)
}

/// Every profile the declaration carries.
pub fn profiles() -> Vec<Value> {
    let arguments = ["scratch", "profiles", "--json"];
    let output = run(&arguments);
    document(&output, &arguments)["profiles"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The leases a host holds right now.
pub fn leases(target: &str) -> Vec<Value> {
    let arguments = ["scratch", "list", "--host", target, "--json"];
    let output = run(&arguments);
    document(&output, &arguments)["leases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One lease's row, when the host still holds it.
pub fn lease_row(target: &str, name: &str) -> Option<Value> {
    leases(target)
        .into_iter()
        .find(|row| row["name"].as_str() == Some(name))
}

/// This machine's own name, for the remote-host preference above.
fn hostname() -> String {
    Command::new("hostname")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_lowercase())
        .unwrap_or_default()
}
