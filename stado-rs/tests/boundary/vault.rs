//! The real Skarbiec broker the dashboard's authorization boundaries read
//! through, and the grants that decide what each verifier can see.
//!
//! Real, not a stand-in: a boundary is a grant and an item set, and a hand
//! written HTTP double can only ever answer what the test already believed.
//! The broker is the one this fleet runs — `SKARBIEC_BIN`, then `PATH`, then
//! `~/.stado/bin/skarbiec` — and it decrypts every read it answers.
//!
//! The item names and token fields are the ones the product's own policies
//! name (`<namespace>-object-api`, `<product>-release-publisher`, field
//! `token`); nothing here is a value that tunes the product.

// The cases use different halves of this fixture, so unused-in-one-case is the
// normal state rather than a finding.
#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

/// The vault owner this fixture creates. A throwaway identity inside the
/// temporary GnuPG home, never the operator's key.
const OWNER: &str = "Boundary Area <boundary-area@example.invalid>";

/// The field every bearer this area seeds lives in, as the object and release
/// verifiers read it.
const FIELD: &str = "token";

/// The Skarbiec item holding one object namespace's bearer, named as
/// `config::parse_object_api_namespaces` requires.
pub fn object_item(namespace: &str) -> String {
    if namespace == "wisent-backend" {
        "wisent-backend-object-client".to_string()
    } else {
        format!("{namespace}-object-api")
    }
}

/// The Skarbiec item holding one product's release-publisher bearer.
pub fn publisher_item(product: &str) -> String {
    format!("{product}-release-publisher")
}

/// The bearer one item holds. Distinct per item, because every verifier
/// refuses an item set whose bearers are not pairwise distinct.
pub fn bearer(item: &str) -> String {
    format!("{item}-bearer")
}

/// The real `skarbiec` binary this fleet runs.
fn binary() -> PathBuf {
    if let Some(configured) = std::env::var_os("SKARBIEC_BIN") {
        let path = PathBuf::from(configured);
        assert!(path.is_file(), "SKARBIEC_BIN is not a file: {path:?}");
        return path;
    }
    if let Ok(found) = Command::new("/usr/bin/which").arg("skarbiec").output() {
        let path = PathBuf::from(String::from_utf8_lossy(&found.stdout).trim().to_string());
        if path.is_file() {
            return path;
        }
    }
    let home = PathBuf::from(std::env::var_os("HOME").expect("the caller has a HOME"));
    let installed = home.join(".stado/bin/skarbiec");
    assert!(
        installed.is_file(),
        "no real Skarbiec broker is available: set SKARBIEC_BIN, put skarbiec on PATH, or install \
         it at {installed:?}"
    );
    installed
}

pub struct Vault {
    home: PathBuf,
    gnupg: PathBuf,
    vault: PathBuf,
    port: u16,
    /// How this broker mints a consumer grant, asked of the broker itself.
    mint: Vec<String>,
    server: Child,
}

/// The subcommand this broker mints consumer grants with.
///
/// `grant issue` replaced `token-mint`, and both are in the fleet tonight —
/// the installed 0.2.40 answers `unknown command: grant` and a build of
/// origin/main answers `unknown command: token-mint`. The broker is asked
/// which one it serves rather than told, because a fixture that names one
/// verb decides for itself which broker the fleet runs.
fn mint_verb(binary: &Path) -> Vec<String> {
    let listed = Command::new(binary)
        .arg("help")
        .output()
        .expect("the real Skarbiec broker lists its commands");
    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    if listed.contains("\"grant\"") {
        return vec!["grant".to_string(), "issue".to_string()];
    }
    if listed.contains("\"token-mint\"") {
        return vec!["token-mint".to_string()];
    }
    let version = Command::new(binary)
        .arg("version")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    panic!(
        "the Skarbiec broker at {binary:?} serves neither `grant issue` nor `token-mint`, so this \
         area cannot provision a verifier grant. It reported: {version}"
    );
}

impl Vault {
    /// Initialise a vault inside `home`, seed one `token` item per name, and
    /// serve it on a loopback port.
    pub fn start(home: &Path, items: &[String]) -> Self {
        let gnupg = home.join("gnupg");
        fs::create_dir_all(&gnupg).expect("create the temporary GnuPG home");
        fs::set_permissions(&gnupg, fs::Permissions::from_mode(0o700))
            .expect("the GnuPG home takes owner-only mode");
        let vault = home.join("skarbiec.json");
        let mut fixture = Self {
            home: home.to_path_buf(),
            gnupg,
            vault,
            port: reserved_port(),
            mint: mint_verb(&binary()),
            server: Command::new("/usr/bin/true")
                .spawn()
                .expect("the placeholder child starts"),
        };
        fixture.run(&["init", OWNER], None);
        for item in items {
            fixture.seed(item);
        }
        fixture.server = fixture.serve();
        fixture
    }

    /// Write one item carrying its own bearer.
    pub fn seed(&self, item: &str) {
        let payload = serde_json::json!({
            "schema": "skarbiec.item.v2",
            "kind": "token",
            "fields": {FIELD: bearer(item)},
            "context": {"service": "stado-boundary-area"},
        });
        self.run(
            &["set-json", item, "--type", "token"],
            Some(&payload.to_string()),
        );
    }

    /// Mint a grant for `consumer` covering exactly `items`, and write it to
    /// `path` with the owner-only mode `skarbiec::read_grant` requires.
    ///
    /// `--replace-capabilities` is what an operator repairing an incomplete
    /// grant passes: the broker refuses to widen a consumer's capabilities
    /// silently, so re-provisioning one says so. Overwriting the grant file
    /// in place is the repair itself — every verifier grant is declared
    /// `RereadPerRequest`, so the next validation reads the new file with
    /// nothing restarted.
    pub fn grant(&self, consumer: &str, items: &[String], path: &Path) {
        let capabilities = items
            .iter()
            .map(|item| format!("read:{item}#{FIELD}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut argv = self.mint.clone();
        argv.extend([
            consumer.to_string(),
            "--replace-capabilities".to_string(),
            "--capabilities".to_string(),
            capabilities,
        ]);
        let minted = self.run(&argv.iter().map(String::as_str).collect::<Vec<_>>(), None);
        let grant: serde_json::Value =
            serde_json::from_slice(&minted).expect("the minted grant is JSON");
        let token = grant["token"].as_str().expect("the grant carries a token");
        fs::write(path, token).expect("write the grant file");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .expect("the grant file takes owner-only mode");
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(binary());
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("GNUPGHOME", &self.gnupg)
            .env(
                "PATH",
                std::env::var("PATH").expect("the caller has a PATH"),
            )
            .env("SKARBIEC_VAULT_FILE", &self.vault)
            .env(
                "SKARBIEC_AUDIT_FILE",
                self.home.join("skarbiec-audit.jsonl"),
            );
        command
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> Vec<u8> {
        let mut child = self
            .command()
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the real Skarbiec broker runs");
        if let Some(body) = stdin {
            child
                .stdin
                .as_mut()
                .expect("stdin is piped")
                .write_all(body.as_bytes())
                .expect("the broker accepts the payload");
        }
        let out = child.wait_with_output().expect("the broker exits");
        assert!(
            out.status.success(),
            "real Skarbiec refused `{}`: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        out.stdout
    }

    fn serve(&self) -> Child {
        let stdout = fs::File::create(self.home.join("skarbiec.out")).expect("broker log");
        let stderr = fs::File::create(self.home.join("skarbiec.err")).expect("broker error log");
        let mut server = self
            .command()
            .args(["serve", "--port", &self.port.to_string()])
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .expect("the real Skarbiec broker starts");
        for _ in 0..250 {
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return server;
            }
            if server.try_wait().expect("the broker is waitable").is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "the real Skarbiec broker did not answer on {}: {}",
            self.port,
            fs::read_to_string(self.home.join("skarbiec.err")).unwrap_or_default()
        );
    }
}

/// A loopback port nothing is listening on. The listener is dropped, so the
/// broker — or nobody, for the cases whose subject is an unreachable vault —
/// claims it next.
pub fn reserved_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("a free loopback port exists")
        .local_addr()
        .expect("the probe has an address")
        .port()
}

impl Drop for Vault {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = Command::new("gpgconf")
            .args([
                "--homedir",
                &self.gnupg.to_string_lossy(),
                "--kill",
                "gpg-agent",
            ])
            .output();
    }
}
