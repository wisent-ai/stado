//! `stado secrets put|get` against the real Skarbiec binary.
//!
//! These tests are ignored by the ordinary Stado suite because the Skarbiec
//! executable is a separate product artifact. Run them with
//! `SKARBIEC_TEST_BIN=/path/to/skarbiec cargo test --test secrets -- --ignored`.
//! Every process uses an isolated HOME, GnuPG home, vault, token, storage root,
//! and loopback port; no operator configuration or vault is reachable.

use std::fs::{self, File};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

struct SkarbiecFixture {
    root: PathBuf,
    gnupg: PathBuf,
    vault: PathBuf,
    token: PathBuf,
    storage: PathBuf,
    skarbiec: PathBuf,
    port: u16,
    server: Option<Child>,
}

impl SkarbiecFixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock follows the Unix epoch")
            .as_nanos();
        // GnuPG creates Unix sockets below GNUPGHOME. Keep this deliberately
        // short so macOS's AF_UNIX path limit cannot break key generation.
        let root = PathBuf::from("/private/tmp").join(format!(
            "stsb{:x}{:08x}",
            std::process::id(),
            unique & 0xffff_ffff
        ));
        let gnupg = root.join("g");
        let storage = root.join("storage");
        fs::create_dir_all(&gnupg).expect("create isolated GnuPG home");
        fs::create_dir_all(&storage).expect("create isolated Stado storage");
        fs::set_permissions(&gnupg, fs::Permissions::from_mode(0o700))
            .expect("protect isolated GnuPG home");

        let skarbiec = PathBuf::from(
            std::env::var("SKARBIEC_TEST_BIN")
                .expect("SKARBIEC_TEST_BIN must name the real Skarbiec executable"),
        );
        assert!(skarbiec.is_file(), "SKARBIEC_TEST_BIN names no file");
        let port = TcpListener::bind("127.0.0.1:0")
            .expect("reserve loopback port")
            .local_addr()
            .expect("read loopback address")
            .port();
        let fixture = Self {
            vault: root.join("vault.json"),
            token: root.join("stado-token"),
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

    fn command(&self, executable: &Path) -> Command {
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

    fn skarbiec(&self, args: &[&str]) -> Output {
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

    fn stado(&self, args: &[&str], body: Option<&str>) -> Output {
        let mut command = self.command(Path::new(env!("CARGO_BIN_EXE_stado")));
        command
            .args(args)
            .env("STADO_CONFIG", self.root.join("no-such-config.json"))
            .env("STADO_CREDENTIALS_STORE", "skarbiec")
            .env("SKARBIEC_BIN", &self.skarbiec)
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

    fn seed_login(&self) {
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

    fn grant_username(&self) {
        let minted = self.skarbiec(&[
            "token-mint",
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

    fn start_server(&mut self) {
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
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assert_success(output: &Output, action: &str) {
    assert!(
        output.status.success(),
        "{action} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires SKARBIEC_TEST_BIN pointing at the separately built Skarbiec product"]
fn secrets_put_writes_a_typed_item_to_real_skarbiec() {
    let fixture = SkarbiecFixture::new();
    let put = fixture.stado(
        &["secrets", "put", "stado-cli-login", "--type", "login"],
        Some(r#"{"username":"alice","password":"not-returned"}"#),
    );
    assert_success(&put, "stado secrets put");
    assert_eq!(
        String::from_utf8_lossy(&put.stdout),
        "stored credential item \"stado-cli-login\" as \"login\"\n"
    );

    let stored = fixture.skarbiec(&["get", "stado-cli-login"]);
    assert_success(&stored, "read fixture state with Skarbiec");
    let document: Value = serde_json::from_slice(&stored.stdout).expect("stored item is JSON");
    assert_eq!(document["kind"], "login");
    assert_eq!(document["fields"]["username"], "alice");
    assert_eq!(document["fields"]["password"], "not-returned");
}

#[test]
#[ignore = "requires SKARBIEC_TEST_BIN pointing at the separately built Skarbiec product"]
fn secrets_get_reads_only_the_granted_field_from_real_skarbiec() {
    let mut fixture = SkarbiecFixture::new();
    fixture.seed_login();
    fixture.grant_username();
    fixture.start_server();

    let get = fixture.stado(
        &["secrets", "get", "stado-cli-login", "--field", "username"],
        None,
    );
    assert_success(&get, "stado secrets get");
    assert_eq!(String::from_utf8_lossy(&get.stdout), "alice\n");

    let refused = fixture.stado(
        &["secrets", "get", "stado-cli-login", "--field", "password"],
        None,
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("consumer not authorized to read item field"),
        "unexpected refusal: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
}
