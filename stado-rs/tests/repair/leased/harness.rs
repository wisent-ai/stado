//! The lease the declared-repair cases drive against, and the reads that are
//! not the command under test.
//!
//! Split out of `leased.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself, the same way `fixture.rs` was
//! split out of `main.rs`.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::fixture::{hostname, stderr, stdout, DECLARATION};

/// One lease at a time. `scratch create` reaps a host's expired leases before
/// taking a new one, so two unsynchronized cases on one host would have each
/// other's leases swept and blame the reaper for working.
pub static HOST: Mutex<()> = Mutex::new(());

/// The service label the fleet's own read-only allowlist can look for inside
/// a managed account's home: `host exec … -- ls -l
/// .stado/services/weles-admission` is a fixed entry that takes no operator
/// path. The declaration below names that label so the tree the registry
/// declares and the tree the second read looks at are one tree.
const DECLARED_SERVICE: &str = "weles-admission";
const DECLARED_UNIT: &str = "com.wisent.weles-admission";
pub const DECLARED_TREE: &str = ".stado/services/weles-admission";
/// SHA-256 of zero bytes: a real digest for an artifact the lease never
/// installs, so the declaration is honest about naming nothing on disk.
const EMPTY_DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// A version no host runs, so a `declared_version` in the report can only
/// have come from the document this case wrote.
pub const DECLARED_VERSION: &str = "0.0.0-leased";
const TTL: &str = "15m";
/// The exit status the capability refuses with.
pub const REFUSED: i32 = 1;

/// The built binary with the caller's environment intact: the fleet
/// configuration a lease needs to resolve its host is the operator's, and
/// this area deliberately does not fake it.
pub fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .output()
        .unwrap_or_else(|exc| panic!("stado {arguments:?} did not start: {exc}"))
}

/// The binary driven through one lease's emitted registry and nothing else.
pub fn leased(root: &str, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", root)
        .env("STADO_CONFIG", Path::new(root).join("no-such-config.json"))
        .output()
        .unwrap_or_else(|exc| panic!("stado {arguments:?} did not start: {exc}"))
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

/// A lease, and the disposable machine state behind it.
pub struct Lease {
    pub name: String,
    pub target: String,
    pub platform: String,
    pub root: String,
    pub home: String,
    pub registry: String,
    pub ssh: String,
    released: bool,
}

/// Take one, on a host the fleet itself says a lease may be taken on. A case
/// that cannot get a host has not passed, so the fleet's own refusal is the
/// failure: never a skip, and never an isolated temporary directory standing
/// in for a machine.
pub fn take_lease() -> Lease {
    let arguments = ["scratch", "hosts", "--json"];
    let listed = run(&arguments);
    let hosts = document(&listed, &arguments);
    let rows = hosts["hosts"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the fleet's host report carries no hosts array: {hosts}"));
    let local = hostname().to_lowercase();
    let mut eligible: Vec<&Value> = rows
        .iter()
        .filter(|row| row["eligible"].as_bool().unwrap_or_default())
        .collect();
    assert!(
        !eligible.is_empty(),
        "no registry target is leasable, so this run is blocked rather than passed: {hosts}"
    );
    // A machine other than this one first: leasing here would put the account
    // beside the operator's own login instead of over the channel every other
    // caller takes.
    let remote = eligible
        .iter()
        .position(|row| {
            let target = row["target"].as_str().unwrap_or_default().to_lowercase();
            !local.starts_with(&target) && !target.starts_with(&local)
        })
        .unwrap_or_default();
    let chosen = eligible.swap_remove(remote);
    let target = chosen["target"]
        .as_str()
        .expect("a target name")
        .to_string();
    let profile = chosen["profile"].as_str().expect("a profile").to_string();
    let platform = chosen["release_platform"]
        .as_str()
        .expect("a release platform")
        .to_string();

    let arguments = [
        "scratch",
        "create",
        "--host",
        &target,
        "--profile",
        &profile,
        "--ttl",
        TTL,
        "--json",
    ];
    let created = run(&arguments);
    let report = document(&created, &arguments);
    assert_eq!(report["status"], "leased");
    assert_eq!(report["account"], "created");
    Lease {
        name: report["name"].as_str().expect("a lease name").to_string(),
        target,
        platform,
        root: report["storage_root"]
            .as_str()
            .expect("a storage root")
            .to_string(),
        home: report["home_path"].as_str().expect("a home").to_string(),
        registry: report["registry_path"]
            .as_str()
            .expect("a registry path")
            .to_string(),
        ssh: report["ssh"]
            .as_str()
            .expect("an ssh destination")
            .to_string(),
        released: false,
    }
}

impl Lease {
    /// The absolute path of the declared service's program, inside this
    /// lease's own home and nowhere else.
    fn program(&self) -> String {
        format!("{}/{DECLARED_TREE}/current/program", self.home)
    }

    /// Declare that service in the lease's own emitted registry: the host
    /// entry states the unit and the program, the directory states the route
    /// and the immutable source it would be installed from, and the target
    /// declares the version its own `~/.stado/bin/stado` is held to.
    pub fn declare_service(&self) {
        let text = std::fs::read_to_string(&self.registry).expect("the emitted registry is read");
        let mut registry: Value = serde_json::from_str(&text).expect("the registry is JSON");
        let name = self.name.as_str();
        registry["targets"][0]["services"] = json!([{
            "name": DECLARED_UNIT,
            "program": self.program(),
        }]);
        registry["targets"][0]["managed_versions"] = json!({ "stado": DECLARED_VERSION });
        registry["service_directory"] = json!({
            "authority": {
                "target": name,
                "command": format!("{}/.stado/bin/stado", self.home),
            },
            "generation": u64::from(u8::MIN) + 1,
            "services": { DECLARED_SERVICE: {
                "active_host": name,
                "managed_service": DECLARED_UNIT,
                "endpoints": { name: { "url": "http://127.0.0.1:8788" } },
                "consumers": { name: { "capabilities": ["invoke"] } },
                "declaration": {
                    "source": {
                        "artifact": "scratch/weles-admission.tar.gz",
                        "sha256": EMPTY_DIGEST,
                    },
                    "run": { "program": self.program() },
                },
            }},
        });
        std::fs::write(
            &self.registry,
            serde_json::to_vec_pretty(&registry).expect("the registry serialises"),
        )
        .expect("the emitted registry is rewritten");
    }

    /// What the leased account's home holds, read by the fleet's own
    /// allowlist over the same channel — never by the command under test.
    pub fn probe(&self, words: &[&str]) -> Value {
        let mut arguments = vec!["host", "exec", &self.name, "--json", "--"];
        arguments.extend_from_slice(words);
        let probed = leased(&self.root, &arguments);
        serde_json::from_str(&stdout(&probed))
            .unwrap_or_else(|exc| panic!("host exec {words:?} printed no receipt: {exc}"))
    }

    /// Give the machine back, and hand over what the destroy reported.
    pub fn release(&mut self) -> Value {
        let arguments = [
            "scratch",
            "destroy",
            &self.name,
            "--host",
            &self.target,
            "--json",
        ];
        let destroyed = run(&arguments);
        self.released = true;
        document(&destroyed, &arguments)
    }
}

/// A case that leaves a lease behind is a defect in the case, including when
/// an assertion took the case out before it got to the destroy.
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.released {
            let _ = run(&[
                "scratch",
                "destroy",
                &self.name,
                "--host",
                &self.target,
                "--json",
            ]);
        }
    }
}

/// The destroy left nothing behind: the account, its home, the record, and
/// the store root the emitted registry lived in.
pub fn assert_absences(report: &Value) {
    assert_eq!(report["status"], "destroyed");
    assert_eq!(report["account"], "absent");
    assert_eq!(report["home"], "absent");
    assert_eq!(report["record"], "absent");
    assert_eq!(report["storage_root"], "removed");
}

/// The ordered repair rows the compiled catalogue declares for one service.
pub fn declared_steps(root: &str, service: &str) -> Vec<Value> {
    let arguments = ["repair", "list", "--service", service, "--json"];
    let listed = leased(root, &arguments);
    let report = document(&listed, &arguments);
    assert_eq!(report["declaration"], DECLARATION);
    let services = report["services"].as_array().expect("declared services");
    assert_eq!(services[0]["name"], service);
    services[0]["repair"]
        .as_array()
        .cloned()
        .expect("the service declares repair rows")
}
