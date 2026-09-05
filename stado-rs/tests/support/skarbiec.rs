use std::fs::{self, File};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

pub fn real_skarbiec_binary() -> PathBuf {
    if let Some(configured) = std::env::var_os("SKARBIEC_TEST_BIN") {
        let configured = PathBuf::from(configured);
        assert!(
            executable_file(&configured),
            "SKARBIEC_TEST_BIN is not an executable file: {}",
            configured.display()
        );
        return configured;
    }

    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    let installed = home.join(".stado/bin/skarbiec");
    if executable_file(&installed) {
        return installed;
    }

    let (platform, asset, expected_sha256) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => (
            "darwin-arm64",
            "skarbiec-v0.2.37-darwin-arm64.tar.gz",
            "d113acc0d831bbefdce0308dbd311e5a6d14c8f9581c962abf380b3c2343743b",
        ),
        ("linux", "x86_64") => (
            "linux-amd64",
            "skarbiec-v0.2.37-linux-amd64.tar.gz",
            "45dc3869f869c347038cc97f3d454bf40f889219152c92862652c0c9e1166c89",
        ),
        (os, arch) => panic!("no real Skarbiec release is pinned for {os}-{arch}"),
    };
    let cache = home.join(".cache/probierz/skarbiec/v0.2.37").join(platform);
    let binary = cache.join("skarbiec");
    if executable_file(&binary) {
        return binary;
    }
    fs::create_dir_all(&cache).unwrap();

    let url = format!("https://github.com/wisent-ai/skarbiec/releases/download/v0.2.37/{asset}");
    let bytes = tokio::runtime::Runtime::new().unwrap().block_on(async {
        reqwest::get(&url)
            .await
            .unwrap_or_else(|error| panic!("downloading real Skarbiec failed: {error}"))
            .error_for_status()
            .unwrap_or_else(|error| panic!("downloading real Skarbiec failed: {error}"))
            .bytes()
            .await
            .unwrap_or_else(|error| panic!("reading real Skarbiec archive failed: {error}"))
    });
    assert_eq!(
        hex::encode(Sha256::digest(&bytes)),
        expected_sha256,
        "downloaded real Skarbiec archive has the wrong digest"
    );

    let mut archive = tempfile::NamedTempFile::new_in(&cache).unwrap();
    archive.write_all(&bytes).unwrap();
    let decoder = flate2::read::GzDecoder::new(File::open(archive.path()).unwrap());
    tar::Archive::new(decoder).unpack(&cache).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        executable_file(&binary),
        "real Skarbiec archive contains no executable"
    );
    binary
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
    pub token: PathBuf,
    port: u16,
    server: Child,
}

impl SkarbiecFixture {
    pub fn start(
        home: &Path,
        items: &[SkarbiecItem],
        consumer: &str,
        capabilities: &str,
        token_name: &str,
    ) -> Self {
        let binary = real_skarbiec_binary();
        let scratch = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".stado/work");
        fs::create_dir_all(&scratch).unwrap();
        let gnupg = tempfile::Builder::new()
            .prefix("skarbiec-fixture-gpg-")
            .tempdir_in(scratch)
            .unwrap();
        fs::set_permissions(gnupg.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let vault = home.join("skarbiec.json");
        let token = home.join(token_name);
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
        let minted = command(
            &["token-mint", consumer, "--capabilities", capabilities],
            None,
        );
        assert!(
            minted.status.success(),
            "real Skarbiec grant failed: {}",
            String::from_utf8_lossy(&minted.stderr)
        );
        let grant: Value = serde_json::from_slice(&minted.stdout).unwrap();
        fs::write(&token, grant["token"].as_str().unwrap()).unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();

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

    pub fn start_release(home: &Path, private_key: &Path) -> Self {
        use base64::Engine;

        let encoded =
            base64::engine::general_purpose::STANDARD.encode(fs::read(private_key).unwrap());
        let item = SkarbiecItem::new(
            "ci-release-signing",
            "key-pair",
            json!({
                "schema": "skarbiec.item.v2",
                "kind": "key-pair",
                "fields": {"private_key": encoded},
                "context": {"service": "stado-release"}
            }),
        );
        Self::start(
            home,
            &[item],
            "stado-release-coordinator",
            "read:ci-release-signing#private_key",
            "release-signing-grant",
        )
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
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
