//! The disposable target the leased cases drive, and everything they need to
//! address it.
//!
//! Shaped after `tests/scratch/fleet.rs`: the fleet's own answer to where a
//! lease may be taken, one host at a time, and a failure rather than a skip
//! when the answer is nowhere. What this adds is the guard — a lease that
//! destroys itself when the case is over, and gives the account back through
//! `Drop` even when the case panicked, so no run can leave one behind.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard};

use serde_json::{json, Value};

/// One lease at a time. `scratch create` reaps the host's expired leases
/// before it takes a new one, so two cases leasing at once would sweep each
/// other's account and blame the reaper for working.
static HOST: Mutex<()> = Mutex::new(());

/// How long a lease is asked for. Every case destroys its own well inside
/// that; the lifetime is only the reaper's backstop for a case that dies.
const LEASE_TTL: &str = "15m";

/// The release build scratch root the `build_scratch` stage sweeps, relative
/// to the target account's home (`deploy::host_reclaim::BUILD_WORK_ROOT`).
pub const BUILD_WORK_ROOT: &str = ".stado/build-work";
/// Where an applied reclamation records itself, and the directory it creates
/// to do so, relative to that same home (`deploy::host_reclaim::AUDIT_LOG`).
pub const AUDIT_LOG: &str = ".stado/audit/host-reclaim.jsonl";
pub const AUDIT_ROOT: &str = ".stado/audit";
/// The janitor's own state document, relative to that home
/// (`providers::local::disk_cleanup::state_relative_path`).
pub const JANITOR_STATE: &str = ".cache/wisent-compute/disk-cleanup-state.json";

/// The payload the reclamation case writes into the leased account's home.
/// Large enough that the bytes show up both in the host's own `df` available
/// figure and in the report's tenth-of-a-gibibyte inventory, small enough to
/// write in about a second.
pub const PAYLOAD_MIB: i64 = 256;
const BLOCK_BYTES: i64 = 1 << 20;
pub const KIB: i64 = 1024;

/// An mtime older than every age gate in the capability, which refuses
/// anything younger than a day.
const AGED_STAMP: &str = "202501010000";
const SSH: &str = "/usr/bin/ssh";

/// Registry-schema configuration for the cleanup policy a reading case adds to
/// its own lease's document. The watermarks are deliberately far above any
/// real machine's free space so the pressure gate is active, and the single
/// cleaner is rooted inside the leased account's own home.
const CHECK_INTERVAL_SECONDS: u32 = 3_600;
const LOW_FREE_GB: u64 = 3_999_999;
const TARGET_FREE_GB: u64 = 4_000_000;
const MAX_BYTES_PER_PASS: u64 = 1_073_741_824;
const MAX_ITEMS_PER_PASS: u32 = 32;
const MAX_SCAN_ITEMS: u32 = 4_096;
const MIN_AGE_SECONDS: u32 = 86_400;
const CACHE_ROOT: &str = "~/build-cache";

/// The built binary with the operator's environment intact: resolving which
/// hosts are leasable is a canonical-registry read, and this area does not
/// fake the fleet it leases from.
pub fn stado(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .output()
        .unwrap_or_else(|exc| panic!("stado {arguments:?} did not start: {exc}"))
}

pub fn said(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The one JSON document a `--json` command printed.
pub fn document(output: &Output, arguments: &[&str]) -> Value {
    assert!(
        output.status.success(),
        "stado {arguments:?} failed: {}{}",
        said(&output.stdout),
        said(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|exc| panic!("stado {arguments:?} printed no JSON document: {exc}"))
}

pub fn text(value: &Value, field: &str) -> String {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} is a string: {value}"))
        .to_string()
}

/// This machine's own name, for the remote-host preference below.
fn hostname() -> String {
    Command::new("hostname")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_lowercase())
        .unwrap_or_default()
}

/// The fleet's own answer to where a lease may be taken, preferring a machine
/// that is not the one running the test: leasing here would create the account
/// beside the operator's own login instead of over the channel every other
/// caller takes. No leasable target is a failure, never a skip.
fn leasable_host() -> (String, String) {
    let arguments = ["scratch", "hosts", "--json"];
    let report = document(&stado(&arguments), &arguments);
    let hosts = report["hosts"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the hosts report carries no hosts array: {report}"));
    let local = hostname();
    let mut eligible: Vec<(String, String)> = hosts
        .iter()
        .filter(|row| row["eligible"].as_bool().unwrap_or_default())
        .map(|row| (text(row, "target"), text(row, "profile")))
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

/// The cleanup policy a case adds to its own lease's emitted document.
fn cleanup_policy() -> Value {
    json!({
        "mode": "enforce",
        "check_interval_seconds": CHECK_INTERVAL_SECONDS,
        "low_free_gb": LOW_FREE_GB,
        "target_free_gb": TARGET_FREE_GB,
        "max_bytes_per_pass": MAX_BYTES_PER_PASS,
        "max_items_per_pass": MAX_ITEMS_PER_PASS,
        "max_scan_items": MAX_SCAN_ITEMS,
        "cleaners": {"build_caches": {"min_age_seconds": MIN_AGE_SECONDS, "root": CACHE_ROOT}},
    })
}

/// A leased target and everything a case needs to address it.
pub struct Lease {
    pub name: String,
    pub home: String,
    host: String,
    storage_root: String,
    registry_path: String,
    ssh: String,
    destroyed: bool,
    _turn: MutexGuard<'static, ()>,
}

impl Lease {
    /// Take one, or fail with the fleet's own words.
    pub fn take() -> Self {
        let turn = HOST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let report = document(&stado(&arguments), &arguments);
        assert_eq!(report["status"], "leased");
        assert_eq!(report["account"], "created");
        Self {
            name: text(&report, "name"),
            home: text(&report, "home_path"),
            host: text(&report, "target"),
            storage_root: text(&report, "storage_root"),
            registry_path: text(&report, "registry_path"),
            ssh: text(&report, "ssh"),
            destroyed: false,
            _turn: turn,
        }
    }

    /// The binary pointed at the registry this lease emitted and at nothing
    /// else: the operator's own configuration sits behind a path that does not
    /// exist, so a regression that stopped reading the emitted document would
    /// fail here rather than quietly address the operator's fleet.
    pub fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage_root)
            .env(
                "STADO_CONFIG",
                Path::new(&self.storage_root).join("no-such-config.json"),
            )
            .output()
            .unwrap_or_else(|exc| panic!("stado {arguments:?} did not start: {exc}"))
    }

    pub fn json(&self, arguments: &[&str]) -> Value {
        document(&self.run(arguments), arguments)
    }

    /// One path inside the leased account's own home.
    pub fn under_home(&self, relative: &str) -> String {
        format!("{}/{relative}", self.home)
    }

    /// `space report` refuses a target that declares no cleanup policy, and
    /// the emitted registry declares none, so a reading case adds one to its
    /// own lease's document with the cleaner rooted inside the leased
    /// account's home. The canonical registry is only ever read.
    pub fn declare_cleanup(&self) {
        let body =
            std::fs::read_to_string(&self.registry_path).expect("the emitted registry is readable");
        let mut declared: Value =
            serde_json::from_str(&body).expect("the emitted registry is JSON");
        declared["targets"][0]["disk_cleanup"] = cleanup_policy();
        std::fs::write(
            &self.registry_path,
            serde_json::to_string_pretty(&declared).expect("the amended registry serializes"),
        )
        .expect("the emitted registry is writable");
    }

    /// Write the payload into the stale build scratch root inside the leased
    /// account's own home, over the destination the lease report named. No
    /// Stado verb puts arbitrary bytes on a host; the capability under test is
    /// what takes them away again, and that is what the case proves.
    pub fn seed_scratch(&self) -> String {
        let script = format!(
            "set -e; tree=$HOME/{BUILD_WORK_ROOT}/release-tree; mkdir -p $tree; \
             dd if=/dev/zero of=$tree/payload.bin bs={BLOCK_BYTES} count={PAYLOAD_MIB}; \
             touch -t {AGED_STAMP} $tree"
        );
        let seeded = Command::new(SSH)
            .args(["-o", "BatchMode=yes", &self.ssh, &script])
            .output()
            .expect("ssh to the leased account starts");
        assert!(
            seeded.status.success(),
            "the payload could not be written into the leased account's home: {}",
            said(&seeded.stderr)
        );
        self.under_home(&format!("{BUILD_WORK_ROOT}/release-tree"))
    }

    /// Give the account back, and assert the three absences the destroy is
    /// contracted to report.
    pub fn destroy(&mut self) {
        let arguments = [
            "scratch", "destroy", &self.name, "--host", &self.host, "--json",
        ];
        let report = document(&stado(&arguments), &arguments);
        self.destroyed = true;
        assert_eq!(report["status"], "destroyed");
        assert_eq!(report["account"], "absent");
        assert_eq!(report["home"], "absent");
        assert_eq!(report["record"], "absent");
    }
}

impl Drop for Lease {
    /// A case that panicked still gives its account back: a run that left a
    /// lease behind is a defect in the case, not in the fleet.
    fn drop(&mut self) {
        if self.destroyed {
            return;
        }
        let _ = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["scratch", "destroy", &self.name, "--host", &self.host])
            .output();
    }
}
