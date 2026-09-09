//! The isolated world a release journey runs in.
//!
//! Every case here owns a temporary `HOME`, a local storage root under it, a
//! Skarbiec vault holding the release signing key, and a committed Rust
//! product that is really compiled by `cargo`. Nothing reads the operator's
//! registry, credential store or fleet: the only paths shared with the machine
//! are the read-only cargo and rustup caches, because compiling a crate
//! without them would download the world.

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use serde_json::json;

use crate::skarbiec_support::{SkarbiecFixture, SkarbiecItem};

impl SkarbiecFixture {
    /// A broker holding the release signing key, granted to the coordinator
    /// consumer alone and reachable only on its own loopback port.
    fn start_release(home: &Path, private_key: &Path) -> Self {
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
            home.join("release-signing-grant"),
            Some((
                "stado-release-coordinator",
                "read:ci-release-signing#private_key",
            )),
            |_, _| {},
        )
    }
}

pub fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("unsupported real release test platform: {os}-{arch}"),
    }
}

pub fn run(command: &mut Command) -> Output {
    let out = command.output().expect("command starts");
    assert!(
        out.status.success(),
        "command failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git(source: &Path, args: &[&str]) {
    run(Command::new("git").current_dir(source).args(args));
}

/// A committed one-binary Rust product with a release declaration: the thing
/// the journey actually builds, signs, publishes and installs.
fn fixture_source(home: &Path, platform: &str, delivery_target: &str) -> PathBuf {
    let source = home.join("source");
    fs::create_dir_all(source.join("src")).unwrap();
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"ci-release-probe\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        source.join("src/main.rs"),
        "fn main() { println!(\"ci-release-probe 1.0.0\"); }\n",
    )
    .unwrap();
    fs::write(
        source.join(".wisent-release.json"),
        serde_json::to_string_pretty(&json!({
            "schema_version": 1,
            "product": "ci-release-probe",
            "releases": true,
            "version_source": {
                "kind": "regex",
                "path": "Cargo.toml",
                "pattern": "(?m)^version\\s*=\\s*\\\"(?P<version>[^\\\"]+)\\\"\\s*$"
            },
            "platforms": {
                (platform): {
                    "runner_platform": platform,
                    "quality": [{
                        "name": "cargo-check",
                        "argv": ["cargo", "check", "--locked"]
                    }],
                    "build": {
                        "argv": ["cargo", "build", "--locked", "--release", "--target-dir", ".wisent-output/target"]
                    },
                    "stage": {
                        "target/release/ci-release-probe": "bin/ci-release-probe"
                    }
                }
            },
            "promotion": {
                "channels": ["candidate", "stable"],
                "reconcile": false
            },
            "deliveries": [{
                "name": "install-on-builder",
                "platform": platform,
                "argv": [
                    "stado", "release", "install-local",
                    "--member", "bin/ci-release-probe",
                    "--name", "ci-release-probe"
                ],
                "required": true,
                "secret_env": {},
                "target": delivery_target
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    git(&source, &["init", "-q"]);
    git(&source, &["config", "user.name", "ci-release"]);
    git(&source, &["config", "user.email", "ci-release@localhost"]);
    run(Command::new("cargo")
        .current_dir(&source)
        .args(["generate-lockfile"]));
    git(&source, &["add", "."]);
    git(&source, &["commit", "-qm", "release source"]);
    source
}

pub struct ReleaseFixture {
    pub platform: &'static str,
    pub storage: PathBuf,
    pub source: PathBuf,
    vault: SkarbiecFixture,
    home: tempfile::TempDir,
}

impl ReleaseFixture {
    /// Build the whole world: keys, vault, registry and committed source.
    pub fn start(
        prefix: &str,
        delivery_target: &str,
        recovery_target: Option<(&str, &str)>,
    ) -> Self {
        let platform = release_platform();
        let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
        fs::create_dir_all(&run_root).unwrap();
        let home = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(run_root)
            .unwrap();
        let storage = home.path().join("store");
        let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
        std::os::unix::fs::symlink(operator_home.join(".cargo"), home.path().join(".cargo"))
            .unwrap();
        std::os::unix::fs::symlink(operator_home.join(".rustup"), home.path().join(".rustup"))
            .unwrap();
        fs::create_dir_all(&storage).unwrap();
        let source = fixture_source(home.path(), platform, delivery_target);

        let private = home.path().join("release-private");
        let public = home.path().join("release-public");
        let worker_bin = home.path().join(".stado/bin/stado");
        fs::create_dir_all(worker_bin.parent().unwrap()).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_stado"), &worker_bin).unwrap();
        fs::set_permissions(&worker_bin, fs::Permissions::from_mode(0o700)).unwrap();
        run(Command::new(env!("CARGO_BIN_EXE_stado")).args([
            "release",
            "keygen",
            "--private-key",
            private.to_str().unwrap(),
            "--public-key",
            public.to_str().unwrap(),
            "--key-id",
            "ci-release-key",
        ]));
        let public_key = fs::read_to_string(&public).unwrap();
        let vault = SkarbiecFixture::start_release(home.path(), &private);
        crate::registry::write(
            home.path(),
            &storage,
            &public_key,
            platform,
            recovery_target,
        );
        Self {
            platform,
            storage,
            source,
            vault,
            home,
        }
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// A `stado` invocation pointed at this fixture's world and nothing else.
    pub fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", self.home())
            .env("GNUPGHOME", self.vault.gnupg_home())
            .env("SKARBIEC_VAULT_FILE", self.vault.vault_file())
            .env("PATH", std::env::var("PATH").unwrap())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_STADO_STORAGE_NAMESPACE", "ci-release")
            .env("STADO_CONFIG", self.home().join("nonexistent-config.json"))
            .env("WC_SKARBIEC_URL", self.vault.url())
            .env(
                "WC_RELEASE_SIGNING_SKARBIEC_CONSUMER",
                "stado-release-coordinator",
            )
            .env("WC_RELEASE_SIGNING_SKARBIEC_TOKEN_FILE", &self.vault.token)
            .env("WC_VAST_AUTO_LIST", "false")
            .env("STADO_RELEASE_SIGNING_KEY_ITEM", "ci-release-signing")
            .env("STADO_RELEASE_SIGNING_KEY_ID", "ci-release-key");
        let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
        command
            .env("CARGO_HOME", operator_home.join(".cargo"))
            .env("RUSTUP_HOME", operator_home.join(".rustup"));
        command
    }

    /// The builder this machine really is, publishing capacity into the store.
    pub fn spawn_agent(&self) -> Child {
        let out = File::create(self.home().join("agent.out")).unwrap();
        let err = File::create(self.home().join("agent.err")).unwrap();
        self.stado()
            .args(["agent", "--target", "ci-runner"])
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .unwrap()
    }

    /// `stado release submit` for version 1.0.0 on the candidate channel,
    /// writing its report to `<leaf>.out` and `<leaf>.err` under the fixture.
    pub fn spawn_submit(&self, leaf: &str) -> Child {
        let out = File::create(self.home().join(format!("{leaf}.out"))).unwrap();
        let err = File::create(self.home().join(format!("{leaf}.err"))).unwrap();
        self.stado()
            .args([
                "release",
                "submit",
                "--source",
                self.source.to_str().unwrap(),
                "--version",
                "1.0.0",
                "--channel",
                "candidate",
                "--json",
            ])
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .unwrap()
    }

    pub fn read(&self, leaf: &str) -> String {
        fs::read_to_string(self.home().join(leaf)).unwrap_or_default()
    }

    /// The binary the delivery installed, run to prove it is the one built.
    pub fn assert_installed_probe_answers(&self) {
        let installed = self.home().join(".stado/bin/ci-release-probe");
        assert!(installed.exists(), "delivery did not install {installed:?}");
        let output = run(&mut Command::new(&installed));
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "ci-release-probe 1.0.0"
        );
    }
}
