//! The lease, from taken to gone, on a real host.

use std::path::Path;
use std::process::Command;

use super::fleet::{document, host_turn, leasable_host, lease_row, run, stderr, stdout};

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
    let run_root = registry_root();
    let registry = run_root.path().join("registry");
    let registry_arg = registry.to_str().expect("a UTF-8 build directory");
    let arguments = [
        "scratch",
        "create",
        "--host",
        &host.target,
        "--profile",
        &host.profile,
        "--ttl",
        "15m",
        "--root",
        registry_arg,
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
    let marker = Path::new(&root).join("scratch-lease.json");
    let identity = std::fs::read(&marker).expect("the emitted lease identity");
    let mut replaced: serde_json::Value =
        serde_json::from_slice(&identity).expect("the lease identity is JSON");
    replaced["name"] = serde_json::Value::from("scratch-another-lease");
    let replaced = serde_json::to_vec(&replaced).expect("the replaced lease identity");
    std::fs::write(&marker, &replaced).expect("replace only this test's lease identity");
    let refused = run(&arguments);
    assert!(
        !refused.status.success(),
        "a replaced registry root was removed"
    );
    assert_eq!(
        std::fs::read(&marker).expect("the root was preserved"),
        replaced
    );
    assert!(
        lease_row(&host.target, &name).is_some(),
        "a cleanup refusal must retain the remote lease record for retry"
    );
    std::fs::write(&marker, identity).expect("restore this test's lease identity");
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

/// Expiry can be swept by the host's janitor before this caller's sweep.
/// Either path must revoke the disposable target and remove its lease.
#[test]
fn an_expired_lease_is_swept_by_the_reaper() {
    let _turn = host_turn();
    let host = leasable_host();
    let run_root = registry_root();
    let registry = run_root.path().join("registry");
    let registry_arg = registry.to_str().expect("a UTF-8 build directory");
    let arguments = [
        "scratch",
        "create",
        "--host",
        &host.target,
        "--profile",
        &host.profile,
        "--ttl",
        "1m",
        "--root",
        registry_arg,
        "--json",
    ];
    let created = run(&arguments);
    let report = document(&created, &arguments);
    let name = report["name"].as_str().expect("a lease name").to_string();

    let expires_at = report["expires_at"].as_str().expect("a recorded expiry");
    let expired = wait_for_expiry(&host.target, &name, expires_at);
    assert!(
        expired,
        "the lease did not read as expired within {EXPIRY_WAIT_SECONDS} seconds, so the sweep cannot be tested"
    );

    let arguments = ["scratch", "reap", "--host", &host.target, "--json"];
    let previewed = run(&arguments);
    let report = document(&previewed, &arguments);
    if let Some(row) = row_for(&report, &name) {
        assert_eq!(row["action"], "would-destroy");
    }
    assert_eq!(report["destroyed"], 0);
    assert!(
        registry.join("scratch-lease.json").is_file(),
        "a preview removed the local registry"
    );
    let probe = run_root.path().join("revoked-target");
    std::fs::create_dir(&probe).expect("the readback registry directory");
    std::fs::copy(registry.join("registry.json"), probe.join("registry.json"))
        .expect("retain the actual leased target for the post-expiry login probe");

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
    if let Some(row) = row_for(&report, &name) {
        assert_eq!(row["action"], "destroyed");
    }
    assert_eq!(report["failures"], serde_json::json!([]));
    assert!(
        lease_row(&host.target, &name).is_none(),
        "the swept lease is gone from the host"
    );
    let expired_target = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["host", "uptime", &name])
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", &probe)
        .env("STADO_CONFIG", probe.join("no-such-config.json"))
        .output()
        .expect("the expired target is probed through its emitted registry");
    eprintln!(
        "expired target {name}: {:?}\n{}{}",
        expired_target.status,
        stdout(&expired_target),
        stderr(&expired_target)
    );
    assert!(
        !expired_target.status.success(),
        "an expired lease still accepts work"
    );
    let parent = run(&["host", "uptime", &host.target]);
    assert!(
        parent.status.success(),
        "a parent-host outage cannot prove lease revocation: {}",
        stderr(&parent)
    );
}

/// Wait for expiry, permitting the independent host janitor to finish first.
fn wait_for_expiry(target: &str, name: &str, expires_at: &str) -> bool {
    let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at).expect("the recorded expiry");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(EXPIRY_WAIT_SECONDS);
    while std::time::Instant::now() < deadline {
        let row = lease_row(target, name);
        if row.is_none() {
            assert!(
                chrono::Utc::now() >= expires_at,
                "the lease was removed before its expiry"
            );
            return true;
        }
        if row
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

fn registry_root() -> tempfile::TempDir {
    let build = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/scratch-test-runs");
    std::fs::create_dir_all(&build).expect("the scratch test build directory");
    tempfile::tempdir_in(build).expect("an isolated scratch registry parent")
}
