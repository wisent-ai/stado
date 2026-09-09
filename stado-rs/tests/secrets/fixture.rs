//! The isolated vault the secrets cases run against.
//!
//! Every process started here gets its own `HOME`, its own GnuPG home, its own
//! vault file, its own grant file, its own Stado storage root and a loopback
//! port reserved by this process. The operator's vault is unreachable: no
//! command inherits an environment, and `SKARBIEC_VAULT_FILE` names a file
//! this fixture created and deletes.

use std::fs::{self, File};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::skarbiec_support::{real_skarbiec_binary, skarbiec_version};

/// The consumer the Stado control plane reads credentials as, and the one
/// field of the seeded item it is granted.
pub const CONSUMER: &str = "stado-control-plane";
pub const ITEM: &str = "stado-cli-login";

/// How long the broker may take to accept a connection on its reserved port.
const READY_TIMEOUT: Duration = Duration::from_secs(2);
const READY_POLL: Duration = Duration::from_millis(20);

pub fn assert_success(output: &Output, action: &str) {
    assert!(
        output.status.success(),
        "{action} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub struct SkarbiecFixture {
    root: tempfile::TempDir,
    gnupg: PathBuf,
    vault: PathBuf,
    token: PathBuf,
    storage: PathBuf,
    skarbiec: PathBuf,
    port: u16,
    server: Option<Child>,
}

impl SkarbiecFixture {
    pub fn new() -> Self {
        // GnuPG puts Unix sockets below GNUPGHOME, so this root stays short:
        // macOS refuses an AF_UNIX path over 104 bytes. `~/.stado/work` is
        // where this repository's tests already keep their scratch state.
        let scratch = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
            .join(".stado/work");
        fs::create_dir_all(&scratch).expect("create the scratch parent");
        let root = tempfile::Builder::new()
            .prefix("stsb-")
            .tempdir_in(scratch)
            .expect("create an isolated fixture root");
        let gnupg = root.path().join("g");
        let storage = root.path().join("storage");
        fs::create_dir_all(&gnupg).expect("create isolated GnuPG home");
        fs::create_dir_all(&storage).expect("create isolated Stado storage");
        fs::set_permissions(&gnupg, fs::Permissions::from_mode(0o700))
            .expect("protect isolated GnuPG home");

        let port = TcpListener::bind("127.0.0.1:0")
            .expect("reserve loopback port")
            .local_addr()
            .expect("read loopback address")
            .port();
        let fixture = Self {
            vault: root.path().join("vault.json"),
            token: root.path().join("stado-grant"),
            storage,
            gnupg,
            root,
            skarbiec: real_skarbiec_binary(),
            port,
            server: None,
        };
        let init = fixture.skarbiec(&[
            "init",
            "Stado Skarbiec test <stado-skarbiec-test@example.invalid>",
        ]);
        assert_success(&init, "initialize fixture vault");
        fixture
    }

    pub fn home(&self) -> &Path {
        self.root.path()
    }

    fn command(&self, executable: &Path) -> Command {
        let mut command = Command::new(executable);
        command
            .env_clear()
            .env("HOME", self.home())
            .env("GNUPGHOME", &self.gnupg)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("NO_COLOR", "1")
            .env("SKARBIEC_VAULT_FILE", &self.vault)
            .env("SKARBIEC_AUDIT_FILE", self.home().join("audit.jsonl"));
        command
    }

    pub fn skarbiec(&self, args: &[&str]) -> Output {
        self.command(&self.skarbiec)
            .args(args)
            .output()
            .expect("run real Skarbiec binary")
    }

    fn skarbiec_with_stdin(&self, args: &[&str], body: &str) -> Output {
        let mut child = self
            .command(&self.skarbiec)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start real Skarbiec binary");
        child
            .stdin
            .as_mut()
            .expect("Skarbiec stdin")
            .write_all(body.as_bytes())
            .expect("write Skarbiec payload");
        child.wait_with_output().expect("finish Skarbiec command")
    }

    /// A `stado` invocation whose credential store is this fixture's broker.
    ///
    /// `STADO_CREDENTIALS_ADMIN_*` is what `stado secrets get` really reads —
    /// `src/credential_store/mod.rs::admin_credentials` — and naming only the
    /// `WC_SKARBIEC_*` control-plane boundary is why this case used to look
    /// for a grant under `$HOME/.stado/local-operator-skarbiec-token`.
    pub fn stado(&self, args: &[&str], body: Option<&str>) -> Output {
        let mut command = self.command(Path::new(env!("CARGO_BIN_EXE_stado")));
        command
            .args(args)
            .env("STADO_CONFIG", self.home().join("no-such-config.json"))
            .env("STADO_CREDENTIALS_STORE", "skarbiec")
            .env("SKARBIEC_BIN", &self.skarbiec)
            .env("WC_SKARBIEC_URL", format!("http://127.0.0.1:{}", self.port))
            .env("STADO_CREDENTIALS_ADMIN_CONSUMER", CONSUMER)
            .env("STADO_CREDENTIALS_ADMIN_TOKEN_FILE", &self.token)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .stdin(if body.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("run real Stado binary");
        if let Some(body) = body {
            child
                .stdin
                .as_mut()
                .expect("Stado stdin")
                .write_all(body.as_bytes())
                .expect("write Stado payload");
        }
        child.wait_with_output().expect("finish Stado command")
    }

    pub fn seed_login(&self) {
        let payload = json!({
            "schema": "skarbiec.item.v2",
            "kind": "login",
            "fields": {"username": "alice", "password": "not-returned"},
            "context": {"service": "example.invalid"}
        });
        let seeded = self.skarbiec_with_stdin(
            &["set-json", ITEM, "--type", "login"],
            &payload.to_string(),
        );
        assert_success(&seeded, "seed fixture item");
    }

    /// The verb the resolved broker declares for issuing a scoped grant.
    ///
    /// `grant issue` replaced `token-mint` in Skarbiec PR #37; both brokers are
    /// in service on this fleet, and the broker itself answers which one it
    /// speaks, so the case reads that answer instead of guessing.
    fn grant_argv(&self) -> Vec<&'static str> {
        let declared = self.skarbiec(&["help"]);
        assert_success(&declared, "read the broker's declared command surface");
        let surface: Value =
            serde_json::from_slice(&declared.stdout).expect("`help` answers JSON");
        let commands = surface["commands"]
            .as_array()
            .expect("`help` lists commands")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if commands.contains(&"grant") {
            return vec!["grant", "issue"];
        }
        assert!(
            commands.contains(&"token-mint"),
            "the resolved skarbiec reports version {} and declares neither `grant` nor \
             `token-mint`, so it cannot issue the scoped grant this case reads one field with. \
             Build wisent-ai/skarbiec at origin/main with `cargo build --release --locked` and \
             point SKARBIEC_TEST_BIN at target/release/skarbiec.",
            skarbiec_version(&self.skarbiec)
        );
        vec!["token-mint"]
    }

    /// Grant the control plane exactly one field of the seeded item.
    pub fn grant_username(&self) {
        let mut argv = self.grant_argv();
        argv.extend([
            CONSUMER,
            "--capabilities",
            "read:stado-cli-login#username",
        ]);
        let minted = self.skarbiec(&argv);
        assert_success(&minted, "issue the one-field Stado grant");
        let document: Value =
            serde_json::from_slice(&minted.stdout).expect("grant response is JSON");
        let bearer = document["token"]
            .as_str()
            .expect("grant response carries a bearer");
        fs::write(&self.token, bearer).expect("write isolated grant file");
        fs::set_permissions(&self.token, fs::Permissions::from_mode(0o600))
            .expect("protect isolated grant file");
    }

    pub fn start_server(&mut self) {
        let stdout = File::create(self.home().join("serve.out")).expect("create serve stdout");
        let stderr = File::create(self.home().join("serve.err")).expect("create serve stderr");
        let skarbiec = self.skarbiec.clone();
        let port = self.port.to_string();
        let server = self
            .command(&skarbiec)
            .args(["serve", "--port", &port])
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .expect("start real Skarbiec server");
        self.server = Some(server);
        let deadline = std::time::Instant::now() + READY_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return;
            }
            if self
                .server
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten())
                .is_some()
            {
                break;
            }
            thread::sleep(READY_POLL);
        }
        let detail = fs::read_to_string(self.home().join("serve.err")).unwrap_or_default();
        panic!("Skarbiec server did not become ready: {detail}");
    }
}

impl Drop for SkarbiecFixture {
    fn drop(&mut self) {
        if let Some(server) = self.server.as_mut() {
            let _ = server.kill();
            let _ = server.wait();
        }
        let _ = Command::new("gpgconf")
            .args([
                "--homedir",
                self.gnupg.to_str().unwrap_or_default(),
                "--kill",
                "gpg-agent",
            ])
            .output();
    }
}
