// Shared by several test targets; each uses a subset, so unused-in-this-target
// is the normal state rather than a finding.
#![allow(dead_code)]

use std::fs::{self, File};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

/// Where declared route resolution landed, quoted as history rather than
/// compared against: a test keyed to a foreign product's revision goes red
/// the day that product moves, for a reason belonging to neither change.
pub const ROUTE_RESOLUTION_ORIGIN: &str =
    "Skarbiec PR #37, merged as 8e8b0ee44, which also deleted /v1/operator/routes/list";

fn executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// The real `skarbiec` binary: `SKARBIEC_TEST_BIN`, then `SKARBIEC_BIN`, then
/// `PATH`, then `~/.stado/bin/skarbiec`, and deliberately no fifth branch. A
/// test that downloads a release of its own choosing decides for itself which
/// Skarbiec the fleet runs, and a test that skips when none is present goes
/// green because a dependency was absent — the defect this change removes.
///
/// `SKARBIEC_TEST_BIN` is read first because
/// `src/cli/host/machine/releases/platform.rs` exports that name for the test
/// run it prepares; `SKARBIEC_BIN` is what the shipped credential path reads
/// in `src/credential_store/owner/discovery.rs`.
pub fn real_skarbiec_binary() -> PathBuf {
    for declared in ["SKARBIEC_TEST_BIN", "SKARBIEC_BIN"] {
        let Some(configured) = std::env::var_os(declared) else {
            continue;
        };
        let configured = PathBuf::from(configured);
        assert!(
            executable_file(&configured),
            "{declared} names {}, which is not an executable file",
            configured.display()
        );
        return configured;
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join("skarbiec");
            if executable_file(&candidate) {
                return candidate;
            }
        }
    }
    let installed =
        PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".stado/bin/skarbiec");
    assert!(
        executable_file(&installed),
        "no real skarbiec binary: neither SKARBIEC_TEST_BIN nor SKARBIEC_BIN is set, no directory \
         on PATH holds `skarbiec`, and {} does not exist. Check out wisent-ai/skarbiec at \
         origin/main, run `cargo build --release --locked`, and point SKARBIEC_TEST_BIN at \
         target/release/skarbiec. This test does not run without the real broker and does not \
         pretend to.",
        installed.display()
    );
    installed
}

/// The version the resolved broker reports, so a refusal can name it.
pub fn skarbiec_version(binary: &Path) -> String {
    let unnamed = || "no version answer".to_string();
    let Ok(reported) = Command::new(binary).arg("version").output() else {
        return unnamed();
    };
    serde_json::from_slice::<Value>(&reported.stdout)
        .ok()
        .and_then(|document| document["version"].as_str().map(str::to_string))
        .unwrap_or_else(unnamed)
}

/// Fail unless the broker at `url` serves declared route resolution.
///
/// Behavioural, because `--version` cannot answer it: a source build reports
/// a null commit and no release at all.
pub fn require_route_resolution(url: &str) {
    let endpoint = format!("{url}/v1/operator/route/resolve");
    let answered = tokio::runtime::Runtime::new()
        .expect("a runtime for the capability probe")
        .block_on(async {
            match reqwest::Client::new()
                .post(&endpoint)
                .json(&json!({}))
                .send()
                .await
            {
                Ok(response) => response.text().await.unwrap_or_default(),
                Err(error) => panic!("the real Skarbiec broker did not answer {endpoint}: {error}"),
            }
        });
    assert!(
        !answered.contains("unknown operator route"),
        "the resolved skarbiec binary does not serve declared route resolution: it answered \
         {answered}. That capability arrived in {ROUTE_RESOLUTION_ORIGIN}, so an older broker \
         serves neither surface. Build skarbiec at origin/main with `cargo build --release \
         --locked` and point SKARBIEC_BIN at target/release/skarbiec. This is a delivery gap — \
         the installed broker is older than a capability Stado already ships — not a Stado defect."
    );
}

pub struct SkarbiecItem {
    name: String,
    kind: String,
    value: Value,
}

impl SkarbiecItem {
    pub fn new(name: impl Into<String>, kind: impl Into<String>, value: Value) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            value,
        }
    }
}

pub struct SkarbiecFixture {
    gnupg: tempfile::TempDir,
    vault: PathBuf,
    pub token: PathBuf,
    port: u16,
    server: Child,
}

impl SkarbiecFixture {
    pub fn start<F>(
        home: &Path,
        items: &[SkarbiecItem],
        token: PathBuf,
        grant: Option<(&str, &str)>,
        provision: F,
    ) -> Self
    where
        F: FnOnce(&Path, &Path),
    {
        let binary = real_skarbiec_binary();
        let scratch = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".stado/work");
        fs::create_dir_all(&scratch).unwrap();
        let gnupg = tempfile::Builder::new()
            .prefix("skarbiec-fixture-gpg-")
            .tempdir_in(scratch)
            .unwrap();
        fs::set_permissions(gnupg.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let vault = home.join("skarbiec.json");
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let command = |args: &[&str], stdin: Option<&str>| {
            let mut child = Command::new(&binary)
                .args(args)
                .env_clear()
                .env("HOME", home)
                .env("GNUPGHOME", gnupg.path())
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("SKARBIEC_VAULT_FILE", &vault)
                .env("SKARBIEC_AUDIT_FILE", home.join("skarbiec-audit.jsonl"))
                .stdin(if stdin.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            if let Some(body) = stdin {
                child
                    .stdin
                    .as_mut()
                    .unwrap()
                    .write_all(body.as_bytes())
                    .unwrap();
            }
            child.wait_with_output().unwrap()
        };
        let initialized = command(&["init", "Stado test <stado-test@example.invalid>"], None);
        assert!(
            initialized.status.success(),
            "real Skarbiec init failed: {}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        for item in items {
            let seeded = command(
                &["set-json", &item.name, "--type", &item.kind],
                Some(&item.value.to_string()),
            );
            assert!(
                seeded.status.success(),
                "real Skarbiec seed failed for {}: {}",
                item.name,
                String::from_utf8_lossy(&seeded.stderr)
            );
        }
        if let Some((consumer, capabilities)) = grant {
            // `grant issue` is the current verb. It replaced `token-mint` in
            // the same merge that added declared route resolution, so a
            // fixture still spelling the old one is pinned to a broker older
            // than the capability this suite requires.
            let minted = command(
                &["grant", "issue", consumer, "--capabilities", capabilities],
                None,
            );
            assert!(
                minted.status.success(),
                "the resolved skarbiec reports version {} and refused `grant issue`: {}. `grant \
                 issue` replaced `token-mint` in the same merge that added declared route \
                 resolution ({ROUTE_RESOLUTION_ORIGIN}), so a broker that does not know the verb \
                 is older than the capability this suite requires. Build skarbiec at origin/main \
                 with `cargo build --release --locked` and point SKARBIEC_TEST_BIN at \
                 target/release/skarbiec.",
                skarbiec_version(&binary),
                String::from_utf8_lossy(&minted.stderr).trim()
            );
            let grant: Value = serde_json::from_slice(&minted.stdout).unwrap();
            fs::write(&token, grant["token"].as_str().unwrap()).unwrap();
            fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        }
        provision(gnupg.path(), &vault);

        let stdout = File::create(home.join("skarbiec.out")).unwrap();
        let stderr = File::create(home.join("skarbiec.err")).unwrap();
        let server = Command::new(&binary)
            .args(["serve", "--port", &port.to_string()])
            .env_clear()
            .env("HOME", home)
            .env("GNUPGHOME", gnupg.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("SKARBIEC_VAULT_FILE", &vault)
            .env("SKARBIEC_AUDIT_FILE", home.join("skarbiec-audit.jsonl"))
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .unwrap();
        let mut fixture = Self {
            gnupg,
            vault,
            token,
            port,
            server,
        };
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return fixture;
            }
            if fixture.server.try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "real Skarbiec did not become ready: {}",
            fs::read_to_string(home.join("skarbiec.err")).unwrap_or_default()
        );
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn gnupg_home(&self) -> &Path {
        self.gnupg.path()
    }

    pub fn vault_file(&self) -> &Path {
        &self.vault
    }
}

impl Drop for SkarbiecFixture {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = Command::new("gpgconf")
            .args([
                "--homedir",
                self.gnupg.path().to_str().unwrap(),
                "--kill",
                "gpg-agent",
            ])
            .output();
    }
}
