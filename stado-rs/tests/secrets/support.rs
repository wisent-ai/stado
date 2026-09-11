//! The isolated vault and fleet every secrets story runs against, and the
//! readers each story uses to see what was written.

use std::fs::{self, File};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

pub(crate) struct SkarbiecFixture {
    /// Owns the removal of everything below, and — because the name is
    /// drawn at random rather than from a clock reading two stories can
    /// share — guarantees each story its own vault, GnuPG home and storage.
    _run: tempfile::TempDir,
    pub(crate) root: PathBuf,
    pub(crate) gnupg: PathBuf,
    pub(crate) vault: PathBuf,
    pub(crate) token: PathBuf,
    pub(crate) storage: PathBuf,
    pub(crate) skarbiec: PathBuf,
    pub(crate) port: u16,
    pub(crate) server: Option<Child>,
}

impl SkarbiecFixture {
    pub(crate) fn new() -> Self {
        // GnuPG creates Unix sockets below GNUPGHOME. Keep this deliberately
        // short so macOS's AF_UNIX path limit cannot break key generation.
        let runs =
            PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".stado/test-runs");
        fs::create_dir_all(&runs).expect("create the isolated test-run root");
        let run = tempfile::Builder::new()
            .prefix("sts")
            .rand_bytes(8)
            .tempdir_in(&runs)
            .expect("reserve an isolated run directory");
        let root = run.path().to_path_buf();
        let gnupg = root.join("g");
        let storage = root.join("storage");
        fs::create_dir_all(&gnupg).expect("create isolated GnuPG home");
        fs::create_dir_all(&storage).expect("create isolated Stado storage");
        fs::set_permissions(&gnupg, fs::Permissions::from_mode(0o700))
            .expect("protect isolated GnuPG home");

        let skarbiec = std::env::var_os("SKARBIEC_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
                    .join(".stado/bin/skarbiec")
            });
        assert!(
            skarbiec.is_file(),
            "the real Skarbiec binary is required; set SKARBIEC_BIN"
        );
        let port = TcpListener::bind("127.0.0.1:0")
            .expect("reserve loopback port")
            .local_addr()
            .expect("read loopback address")
            .port();
        let fixture = Self {
            vault: root.join("vault.json"),
            token: root.join("stado-token"),
            _run: run,
            root,
            gnupg,
            storage,
            skarbiec,
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

    pub(crate) fn command(&self, executable: &Path) -> Command {
        let mut command = Command::new(executable);
        command
            .env_clear()
            .env("HOME", &self.root)
            .env("GNUPGHOME", &self.gnupg)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("SKARBIEC_VAULT_FILE", &self.vault)
            .env("SKARBIEC_AUDIT_FILE", self.root.join("audit.jsonl"));
        command
    }

    pub(crate) fn skarbiec(&self, args: &[&str]) -> Output {
        self.command(&self.skarbiec)
            .args(args)
            .output()
            .expect("run real Skarbiec binary")
    }

    pub(crate) fn skarbiec_with_stdin(&self, args: &[&str], body: &str) -> Output {
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

    pub(crate) fn stado(&self, args: &[&str], body: Option<&str>) -> Output {
        let mut command = self.command(Path::new(env!("CARGO_BIN_EXE_stado")));
        command
            .args(args)
            .env("STADO_CONFIG", self.root.join("no-such-config.json"))
            .env("STADO_CREDENTIALS_STORE", "skarbiec")
            .env("SKARBIEC_BIN", &self.skarbiec)
            .env("SKARBIEC_LAUNCHER", &self.skarbiec)
            .env(
                "STADO_CREDENTIALS_ADMIN_URL",
                format!("http://127.0.0.1:{}", self.port),
            )
            .env("STADO_CREDENTIALS_ADMIN_CONSUMER", "stado-control-plane")
            .env("STADO_CREDENTIALS_ADMIN_TOKEN_FILE", &self.token)
            .env("WC_SKARBIEC_URL", format!("http://127.0.0.1:{}", self.port))
            .env("WC_SKARBIEC_CONSUMER", "stado-control-plane")
            .env("WC_SKARBIEC_TOKEN_FILE", &self.token)
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

    pub(crate) fn seed_login(&self) {
        let payload = json!({
            "schema": "skarbiec.item.v2",
            "kind": "login",
            "fields": {"username": "alice", "password": "not-returned"},
            "context": {"service": "example.invalid"}
        });
        let seeded = self.skarbiec_with_stdin(
            &["set-json", "stado-cli-login", "--type", "login"],
            &payload.to_string(),
        );
        assert_success(&seeded, "seed fixture item");
    }

    pub(crate) fn grant_username(&self) {
        let minted = self.skarbiec(&[
            "grant",
            "issue",
            "stado-control-plane",
            "--capabilities",
            "read:stado-cli-login#username",
        ]);
        assert_success(&minted, "mint one-field Stado grant");
        let document: Value =
            serde_json::from_slice(&minted.stdout).expect("token response is JSON");
        let bearer = document
            .get("token")
            .and_then(Value::as_str)
            .expect("token response carries bearer");
        fs::write(&self.token, bearer).expect("write isolated token file");
        fs::set_permissions(&self.token, fs::Permissions::from_mode(0o600))
            .expect("protect isolated token file");
    }

    pub(crate) fn start_server(&mut self) {
        let stdout = File::create(self.root.join("serve.out")).expect("create serve stdout");
        let stderr = File::create(self.root.join("serve.err")).expect("create serve stderr");
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
        for _ in 0..100 {
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
            thread::sleep(Duration::from_millis(20));
        }
        let detail = fs::read_to_string(self.root.join("serve.err")).unwrap_or_default();
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
        // `_run` removes the tree itself once this fixture is dropped.
    }
}

pub(crate) fn assert_success(output: &Output, action: &str) {
    assert!(
        output.status.success(),
        "{action} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
