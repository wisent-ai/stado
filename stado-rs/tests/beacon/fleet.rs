//! The isolated fleet one beacon publication runs against: this machine, in a
//! registry, a store and a home of its own.
//!
//! The registry names this machine's own kernel hostname, normalized, so the
//! publisher recognises a document about itself and collects its link block
//! from this machine's real tools. `HOME`, the vault, the keyring and the
//! store all live inside one tempdir; the operator's vault, keyring, registry
//! and fleet store are never read and never written.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use serde_json::{json, Value};

use crate::broker::real_skarbiec;
use crate::listeners::{
    await_listener, free_port, owner_only_file, start_dashboard, start_skarbiec, stop_key_agent,
    Vault, OWNER_ONLY_DIRECTORY, SYSTEM_PATH,
};

/// The registry name of the isolated entry standing for this machine.
pub const TARGET: &str = "beacon-current-host";

const REGISTRY_SCHEMA: u32 = 2;

pub struct Fleet {
    root: tempfile::TempDir,
    home: PathBuf,
    storage: PathBuf,
    gnupg: PathBuf,
    publisher_token: PathBuf,
    api_url: String,
    /// This machine's beacon slug: the leading hostname label, lowercased.
    pub host: String,
    skarbiec: Option<Child>,
    dashboard: Option<Child>,
}

impl Fleet {
    pub fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("beacon-")
            .tempdir()
            .expect("create the isolated beacon root");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        let gnupg = root.path().join("gnupg");
        for directory in [&home, &storage, &gnupg] {
            fs::create_dir_all(directory).expect("create an isolated beacon directory");
        }
        fs::set_permissions(&gnupg, fs::Permissions::from_mode(OWNER_ONLY_DIRECTORY))
            .expect("keep the isolated keyring owner-only");
        let host = hostname();
        fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": REGISTRY_SCHEMA,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "release_platform": release_platform(),
                    "hostnames": [host.clone()],
                    "services": [],
                }],
                "coordinators": [],
            }))
            .unwrap(),
        )
        .expect("write the isolated registry");

        // The credential authority the host-health route reads through: a real
        // vault, a real grant, both created here and thrown away with the case.
        let vault = Vault::provision(
            real_skarbiec(),
            &home,
            &gnupg,
            home.join("beacon-area.vault.json"),
        );
        let publisher_token = root.path().join("publisher-token");
        let verifier_token = root.path().join("verifier-token");
        owner_only_file(&publisher_token, vault.bearer());
        owner_only_file(&verifier_token, vault.grant());

        let skarbiec_port = free_port();
        let skarbiec = start_skarbiec(&vault, skarbiec_port);
        await_listener(skarbiec_port, "the Skarbiec listener");
        let dashboard_port = free_port();
        let dashboard = start_dashboard(
            &home,
            &storage,
            root.path(),
            dashboard_port,
            skarbiec_port,
            &verifier_token,
        );
        await_listener(dashboard_port, "the Stado host-health listener");

        Self {
            root,
            home,
            storage,
            gnupg,
            publisher_token,
            api_url: format!("http://127.0.0.1:{dashboard_port}"),
            host,
            skarbiec: Some(skarbiec),
            dashboard: Some(dashboard),
        }
    }

    /// One `stado` invocation against this isolated machine.
    pub fn run(&self, arguments: &[&str]) -> Output {
        self.run_with_token(arguments, &self.publisher_token)
    }

    /// The same invocation with a different owner-only bearer file, for the
    /// case where the route refuses what it is given.
    pub fn run_with_token(&self, arguments: &[&str], token_file: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("STADO_CONFIG", self.root.path().join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("STADO_HOST_HEALTH_API_URL", &self.api_url)
            .env("STADO_HOST_HEALTH_API_TOKEN_FILE", token_file)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("the built stado binary runs")
    }

    /// Write one beacon document in the shape the collector scripts publish,
    /// and return its path.
    pub fn document(&self, host: &str, reported_at: &str) -> PathBuf {
        self.write(
            &format!("{host}.beacon.json"),
            &serde_json::to_string_pretty(&json!({
                "host": host,
                "reported_at": reported_at,
                "disk": "/dev/disk3s1s1 1.8Ti 9.8Gi 200Gi 5% /",
                "units": {"com.wisent.host-health-beacon": {"state": "loaded"}},
            }))
            .unwrap(),
        )
    }

    pub fn write(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root.path().join(name);
        fs::write(&path, body).expect("write a fixture document");
        path
    }

    pub fn token_file(&self, name: &str, value: &str) -> PathBuf {
        let path = self.root.path().join(name);
        owner_only_file(&path, value);
        path
    }

    /// The beacon object the listener itself wrote into the fleet store.
    pub fn stored(&self, host: &str) -> Option<Value> {
        let bytes = fs::read(self.storage.join(format!("host_health/{host}.json"))).ok()?;
        Some(serde_json::from_slice(&bytes).expect("the stored beacon is JSON"))
    }
}

impl Drop for Fleet {
    fn drop(&mut self) {
        for listener in [self.dashboard.as_mut(), self.skarbiec.as_mut()] {
            if let Some(child) = listener {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        stop_key_agent(&self.gnupg);
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn said(output: &Output) -> String {
    format!("stdout={} stderr={}", stdout(output), stderr(output))
}

/// The kernel's hostname, normalized the way the beacon slug is spelled: the
/// leading label, lowercased.
fn hostname() -> String {
    let reported = Command::new("hostname")
        .env("PATH", SYSTEM_PATH)
        .output()
        .expect("this machine answers hostname");
    let name = String::from_utf8(reported.stdout).expect("the hostname is UTF-8");
    let label = name
        .trim()
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(!label.is_empty(), "this machine reports no hostname label");
    label.to_ascii_lowercase()
}

fn release_platform() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "darwin-arm64"
    } else {
        "darwin-amd64"
    }
}
