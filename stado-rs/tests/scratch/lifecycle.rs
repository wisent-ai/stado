//! The lease, from taken to gone, on a real host.

use std::path::Path;
use std::process::Command;

use super::fleet::{document, home, host_turn, leasable_host, lease_row, run, stderr, stdout};

/// How long the expiry story waits for a minute-long lease to be over. The
/// lease's own lifetime is the minute; this is the margin plus the polling
/// interval, and nothing else is tuned here.
const EXPIRY_WAIT_SECONDS: u64 = 150;
/// Seconds between two reads of the host's leases while waiting.
const POLL_SECONDS: u64 = 10;

/// The whole story: a lease is taken on a host the fleet declares leasable, the
/// emitted registry document is the one a caller uses, a real host command runs
/// through it, and the destroy leaves the account, its home and the record all
/// absent.
#[test]
fn a_lease_is_created_entered_and_destroyed_on_a_leasable_host() {
    let _turn = host_turn();
    let host = leasable_host();
    let arguments = [
        "scratch",
        "create",
        "--host",
        &host.target,
        "--profile",
        &host.profile,
        "--ttl",
        "15m",
        "--json",
    ];
    let created = run(&arguments);
    let report = document(&created, &arguments);
    let name = report["name"]
        .as_str()
        .expect("the lease report names the lease")
        .to_string();
    assert_eq!(report["status"], "leased");
    assert_eq!(report["account"], "created");
    assert_eq!(
        report["verified_login"],
        serde_json::Value::from(name.clone()),
        "create proves the lease can be entered as itself: {report}"
    );
    assert_eq!(
        report["target"],
        serde_json::Value::from(host.target.clone())
    );

    // The document a caller points WC_LOCAL_STORAGE_PATH at, read off disk.
    let root = report["storage_root"]
        .as_str()
        .expect("a storage root")
        .to_string();
    let registry_path = report["registry_path"].as_str().expect("a registry path");
    let declared: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(registry_path).expect("the emitted registry is readable"),
    )
    .expect("the emitted registry is JSON");
    let target = &declared["targets"][0];
    assert_eq!(target["name"], serde_json::Value::from(name.clone()));
    assert_eq!(target["kind"], "local");
    assert_eq!(
        target["ssh_key_target"],
        serde_json::Value::from(host.target.clone()),
        "a leased target authenticates with the key minted for its host: {declared}"
    );
    assert!(
        target["ssh"]
            .as_str()
            .unwrap_or_default()
            .starts_with(&name),
        "the ssh destination logs in as the lease: {declared}"
    );

    // A capability, driven against the disposable target through that document.
    let uptime = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["host", "uptime", &name])
        .env("HOME", home())
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", &root)
        .env("STADO_CONFIG", Path::new(&root).join("no-such-config.json"))
        .output()
        .expect("stado host uptime starts");
    assert!(
        uptime.status.success(),
        "the leased target did not answer a real host command: {}{}",
        stdout(&uptime),
        stderr(&uptime)
    );
    assert!(
        stdout(&uptime).contains("uptime:"),
        "the host's own uptime is what came back: {}",
        stdout(&uptime)
    );

    // The host's account of what it holds.
    let listed = lease_row(&host.target, &name).expect("the host reports the lease it holds");
    assert_eq!(listed["account"], "present");
    assert_eq!(
        listed["profile"],
        serde_json::Value::from(host.profile.clone())
    );
    assert_eq!(listed["expired"], false);

    let arguments = [
        "scratch",
        "destroy",
        &name,
        "--host",
        &host.target,
        "--json",
    ];
    let destroyed = run(&arguments);
    let report = document(&destroyed, &arguments);
    assert_eq!(report["status"], "destroyed");
    assert_eq!(report["account"], "absent");
    assert_eq!(report["home"], "absent");
    assert_eq!(report["record"], "absent");
    assert_eq!(report["storage_root"], "removed");
    assert!(
        report["destroyed_at"]
            .as_str()
            .is_some_and(|stamp| !stamp.is_empty()),
        "the destroy is stamped: {report}"
    );

    assert!(
        lease_row(&host.target, &name).is_none(),
        "the host no longer reports the lease"
    );
    assert!(
        !Path::new(&root).exists(),
        "the emitted registry root went with the lease"
    );
}

/// The safety property: a lease nobody destroys is destroyed anyway once its
/// declared lifetime is over, and the sweep says so before it does it.
#[test]
fn an_expired_lease_is_swept_by_the_reaper() {
    let _turn = host_turn();
    let host = leasable_host();
    let arguments = [
        "scratch",
        "create",
        "--host",
        &host.target,
        "--profile",
        &host.profile,
        "--ttl",
        "1m",
        "--json",
    ];
    let created = run(&arguments);
    let report = document(&created, &arguments);
    let name = report["name"].as_str().expect("a lease name").to_string();

    let expired = wait_for_expiry(&host.target, &name);
    assert!(
        expired,
        "the lease did not read as expired within {EXPIRY_WAIT_SECONDS} seconds, so the sweep cannot be tested"
    );

    let arguments = ["scratch", "reap", "--host", &host.target, "--json"];
    let previewed = run(&arguments);
    let report = document(&previewed, &arguments);
    let row = row_for(&report, &name).expect("the preview names the expired lease");
    assert_eq!(row["action"], "would-destroy");
    assert_eq!(report["destroyed"], 0);
    assert_eq!(
        lease_row(&host.target, &name)
            .map(|row| row["account"].clone())
            .unwrap_or_default(),
        serde_json::Value::from("present"),
        "a preview changes nothing on the host"
    );

    let arguments = [
        "scratch",
        "reap",
        "--host",
        &host.target,
        "--apply",
        "--json",
    ];
    let applied = run(&arguments);
    let report = document(&applied, &arguments);
    let row = row_for(&report, &name).expect("the sweep names the lease it took");
    assert_eq!(row["action"], "destroyed");
    assert_eq!(report["failures"], serde_json::json!([]));
    assert!(
        lease_row(&host.target, &name).is_none(),
        "the swept lease is gone from the host"
    );
}

/// Read the host until its own report calls the lease expired.
fn wait_for_expiry(target: &str, name: &str) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(EXPIRY_WAIT_SECONDS);
    while std::time::Instant::now() < deadline {
        if lease_row(target, name)
            .and_then(|row| row["expired"].as_bool())
            .unwrap_or_default()
        {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_secs(POLL_SECONDS));
    }
    false
}

/// One lease's row inside a sweep report.
fn row_for(report: &serde_json::Value, name: &str) -> Option<serde_json::Value> {
    report["leases"]
        .as_array()?
        .iter()
        .find(|row| row["name"].as_str() == Some(name))
        .cloned()
}
