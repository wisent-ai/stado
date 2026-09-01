//! Real build → release → install proof.
//!
//! The test drives the compiled `stado` binary end to end. It creates a clean
//! committed Rust product, starts a real Stado worker against an isolated
//! local store, runs `stado release submit`, requires the worker to execute
//! `cargo check` and `cargo build --release`, signs and publishes the archive,
//! delivers it through `stado release install-local`, then executes the
//! installed binary and checks its version output. No fleet host or operator
//! registry is read or changed.
use std::fs::{self, File};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("release journey has no platform mapping for {os}-{arch}"),
    }
}

struct SkarbiecFixture {
    gnupg: tempfile::TempDir,
    token: PathBuf,
    port: u16,
    server: Child,
}

impl SkarbiecFixture {
    fn start(home: &Path, private_key: &Path) -> Self {
        use base64::Engine;

        let binary = PathBuf::from(
            std::env::var("SKARBIEC_TEST_BIN")
                .expect("SKARBIEC_TEST_BIN must name the real Skarbiec executable"),
        );
        assert!(binary.is_file(), "SKARBIEC_TEST_BIN names no file");
        let scratch = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".stado/work");
        fs::create_dir_all(&scratch).unwrap();
        let gnupg = tempfile::Builder::new()
            .prefix("release-gpg-")
            .tempdir_in(scratch)
            .unwrap();
        fs::set_permissions(gnupg.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let vault = home.join("skarbiec.json");
        let token = home.join("release-signing-grant");
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
        let initialized = command(
            &[
                "init",
                "Stado release test <stado-release-test@example.invalid>",
            ],
            None,
        );
        assert!(
            initialized.status.success(),
            "real Skarbiec init failed: {}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(fs::read(private_key).unwrap());
        let item = json!({
            "schema": "skarbiec.item.v2",
            "kind": "key-pair",
            "fields": {"private_key": encoded},
            "context": {"service": "stado-release"}
        });
        let seeded = command(
            &["set-json", "ci-release-signing", "--type", "key-pair"],
            Some(&item.to_string()),
        );
        assert!(
            seeded.status.success(),
            "real Skarbiec seed failed: {}",
            String::from_utf8_lossy(&seeded.stderr)
        );
        let minted = command(
            &[
                "token-mint",
                "stado-release-coordinator",
                "--capabilities",
                "read:ci-release-signing#private_key",
            ],
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

    fn url(&self) -> String {
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

fn run(command: &mut Command) -> Output {
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

fn release_env(command: &mut Command, home: &Path, storage: &Path, vault: &SkarbiecFixture) {
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("WC_STADO_STORAGE_NAMESPACE", "ci-release")
        .env("STADO_CONFIG", home.join("nonexistent-config.json"))
        .env("WC_SKARBIEC_URL", vault.url())
        .env(
            "WC_RELEASE_SIGNING_SKARBIEC_CONSUMER",
            "stado-release-coordinator",
        )
        .env("WC_RELEASE_SIGNING_SKARBIEC_TOKEN_FILE", &vault.token)
        .env("WC_VAST_AUTO_LIST", "false")
        .env("STADO_RELEASE_SIGNING_KEY_ITEM", "ci-release-signing")
        .env("STADO_RELEASE_SIGNING_KEY_ID", "ci-release-key");
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    command
        .env("CARGO_HOME", operator_home.join(".cargo"))
        .env("RUSTUP_HOME", operator_home.join(".rustup"));
}

fn fixture_source(home: &Path, platform: &str) -> PathBuf {
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
                "target": ""
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

fn registry(home: &Path, storage: &Path, public_key: &str, platform: &str) {
    let hostname = String::from_utf8(run(Command::new("hostname").arg("-f")).stdout)
        .unwrap()
        .trim()
        .to_ascii_lowercase();
    let document = json!({
        "schema_version": 2,
        "targets": [{
            "name": "ci-runner",
            "kind": "local",
            "ssh": "nobody@127.0.0.1",
            "release_platform": platform,
            "hostnames": [hostname],
            "disk_cleanup": {
                "mode": "off",
                "check_interval_seconds": 300,
                "low_free_gb": 10,
                "target_free_gb": 12,
                "max_bytes_per_pass": 53687091200_u64,
                "max_items_per_pass": 50,
                "max_scan_items": 10000,
                "cleaners": {}
            },
            "services": [{
                "kind": "launchd",
                "name": "ci-release-probe",
                "label": "ci-release-probe",
                "path": home.join("Library/LaunchAgents/ci-release-probe.plist"),
                "unit": ""
            }],
            "slots": 1
        }],
        "service_directory": {
            "authority": {
                "target": "ci-runner",
                "command": env!("CARGO_BIN_EXE_stado")
            },
            "generation": 1,
            "services": {
                "ci-release-probe": {
                    "active_host": "ci-runner",
                    "managed_service": "ci-release-probe",
                    "endpoints": {
                        "ci-runner": {"url": "http://127.0.0.1:1"}
                    },
                    "consumers": {
                        "ci-release": {"capabilities": ["release"]}
                    }
                }
            }
        },
        "release_control": {
            "schema_version": 1,
            "generation": 1,
            "trusted_keys": {"ci-release-key": public_key.trim()},
            "products": {
                "ci-release-probe": {
                    "service": "ci-release-probe",
                    "config_schema": 1,
                    "state_schema": 1,
                    "install_root": home.join(".stado/services/ci-release-probe"),
                    "binary": "bin/ci-release-probe",
                    "launcher": "bin/ci-release-probe",
                    "binary_env": "CI_RELEASE_PROBE_BIN",
                    "port_env": "CI_RELEASE_PROBE_PORT",
                    "runtime_env": "CI_RELEASE_PROBE_RUNTIME",
                    "environment": {},
                    "signing_key_item": "ci-release-signing",
                    "signing_key_id": "ci-release-key",
                    "strategy": {
                        "kind": "replace",
                        "readiness_timeout_seconds": 30,
                        "drain_timeout_seconds": 30,
                        "rollback_window_seconds": 300,
                        "automatic_rollback": false
                    },
                    "targets": {
                        "ci-runner": {
                            "platform": platform,
                            "run_as_user": "ci-release",
                            "home": home,
                            "state_dir": home.join(".stado/release-state"),
                            "runtime_root": home.join(".stado/run"),
                            "logs_root": home.join(".stado/logs"),
                            "readiness_path": "/healthz"
                        }
                    }
                }
            }
        }
    });
    fs::write(
        storage.join("registry.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
}

fn wait_for_capacity(storage: &Path, home: &Path, agent: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let capacity = storage.join("capacity");
        if fs::read_dir(&capacity)
            .ok()
            .and_then(|mut entries| entries.next())
            .is_some()
        {
            return;
        }
        if let Some(status) = agent.try_wait().unwrap() {
            panic!(
                "agent exited before publishing capacity: {status}\nstdout:\n{}\nstderr:\n{}",
                fs::read_to_string(home.join("agent.out")).unwrap_or_default(),
                fs::read_to_string(home.join("agent.err")).unwrap_or_default()
            );
        }
        assert!(Instant::now() < deadline, "agent published no capacity");
        thread::sleep(Duration::from_millis(100));
    }
}

fn store_snapshot(storage: &Path) -> String {
    let mut out = String::new();
    for prefix in ["queue", "running", "failed", "completed", "capacity"] {
        let path = storage.join(prefix);
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            out.push_str(&format!(
                "\n== {prefix}/{} ==\n{}",
                entry.file_name().to_string_lossy(),
                fs::read_to_string(entry.path()).unwrap_or_else(|_| "<binary>".into())
            ));
        }
    }
    out
}

fn wait_for_submit(
    child: &mut Child,
    agent: &mut Child,
    home: &Path,
    storage: &Path,
) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if let Some(status) = agent.try_wait().unwrap() {
            let _ = child.kill();
            panic!(
                "agent exited while release submit waited: {status}\nagent stdout:\n{}\nagent stderr:\n{}",
                fs::read_to_string(home.join("agent.out")).unwrap_or_default(),
                fs::read_to_string(home.join("agent.err")).unwrap_or_default()
            );
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "release submit did not finish within 180 seconds\nsubmit stdout:\n{}\nsubmit stderr:\n{}\nagent stdout:\n{}\nagent stderr:\n{}\nstore:{}",
                fs::read_to_string(home.join("submit.out")).unwrap_or_default(),
                fs::read_to_string(home.join("submit.err")).unwrap_or_default(),
                fs::read_to_string(home.join("agent.out")).unwrap_or_default(),
                fs::read_to_string(home.join("agent.err")).unwrap_or_default(),
                store_snapshot(storage)
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "Probierz supplies the real Skarbiec executable"]
fn a_real_release_builds_publishes_and_installs_its_binary() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-")
        .tempdir_in(run_root)
        .unwrap();
    let storage = home.path().join("store");
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    std::os::unix::fs::symlink(operator_home.join(".cargo"), home.path().join(".cargo")).unwrap();
    std::os::unix::fs::symlink(operator_home.join(".rustup"), home.path().join(".rustup")).unwrap();
    fs::create_dir_all(&storage).unwrap();
    let source = fixture_source(home.path(), platform);

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
    let vault = SkarbiecFixture::start(home.path(), &private);
    registry(home.path(), &storage, &public_key, platform);

    let agent_out = File::create(home.path().join("agent.out")).unwrap();
    let agent_err = File::create(home.path().join("agent.err")).unwrap();
    let mut agent_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut agent_command, home.path(), &storage, &vault);
    let mut agent = agent_command
        .args(["agent", "--target", "ci-runner"])
        .stdout(Stdio::from(agent_out))
        .stderr(Stdio::from(agent_err))
        .spawn()
        .unwrap();
    wait_for_capacity(&storage, home.path(), &mut agent);
    let submit_out = File::create(home.path().join("submit.out")).unwrap();
    let submit_err = File::create(home.path().join("submit.err")).unwrap();
    let mut submit = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut submit, home.path(), &storage, &vault);
    let mut submit = submit
        .args([
            "release",
            "submit",
            "--source",
            source.to_str().unwrap(),
            "--version",
            "1.0.0",
            "--channel",
            "candidate",
            "--json",
        ])
        .stdout(Stdio::from(submit_out))
        .stderr(Stdio::from(submit_err))
        .spawn()
        .unwrap();
    let status = wait_for_submit(&mut submit, &mut agent, home.path(), &storage);
    let result = Output {
        status,
        stdout: fs::read(home.path().join("submit.out")).unwrap(),
        stderr: fs::read(home.path().join("submit.err")).unwrap(),
    };
    let _ = agent.kill();
    let _ = agent.wait();
    assert!(
        result.status.success(),
        "release submit failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let release: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(release["state"], "completed");
    assert_eq!(release["platforms"][platform]["state"], "published");
    assert_eq!(
        release["deliveries"]["install-on-builder"]["state"],
        "passed"
    );

    let installed = home.path().join(".stado/bin/ci-release-probe");
    assert!(installed.exists(), "delivery did not install {installed:?}");
    let output = run(&mut Command::new(&installed));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "ci-release-probe 1.0.0"
    );
    println!("verified release platform={platform}; installed=ci-release-probe 1.0.0");
}
