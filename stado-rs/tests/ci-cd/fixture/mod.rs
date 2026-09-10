use super::*;

mod wait;
pub(super) use wait::*;
impl SkarbiecFixture {
    pub(crate) fn start_release(home: &Path, private_key: &Path) -> Self {
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

pub(crate) fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("unsupported real release test platform: {os}-{arch}"),
    }
}

pub(crate) fn run(command: &mut Command) -> Output {
    let out = command.output().expect("command starts");
    assert!(
        out.status.success(),
        "command failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

pub(crate) fn git(source: &Path, args: &[&str]) {
    run(Command::new("git").current_dir(source).args(args));
}

pub(crate) fn release_env(
    command: &mut Command,
    home: &Path,
    storage: &Path,
    vault: &SkarbiecFixture,
) {
    command
        .env_clear()
        .env("HOME", home)
        .env("GNUPGHOME", vault.gnupg_home())
        .env("SKARBIEC_VAULT_FILE", vault.vault_file())
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

pub(crate) fn fixture_source(home: &Path, platform: &str, delivery_target: &str) -> PathBuf {
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

pub(crate) fn registry(
    home: &Path,
    storage: &Path,
    public_key: &str,
    platform: &str,
    recovery_target: Option<(&str, &str)>,
) {
    let hostname = String::from_utf8(run(Command::new("hostname").arg("-f")).stdout)
        .unwrap()
        .trim()
        .to_ascii_lowercase();
    let mut document = json!({
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
                "low_free_gb": 1,
                "target_free_gb": 2,
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
            }]
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
    if let Some((name, hostname)) = recovery_target {
        document["targets"]
            .as_array_mut()
            .expect("registry targets are an array")
            .push(json!({
                "name": name,
                "kind": "local",
                "ssh": "nobody@offline-recovery.invalid",
                "release_platform": platform,
                "hostnames": [hostname],
                "disk_cleanup": {
                    "mode": "off",
                    "check_interval_seconds": 300,
                    "low_free_gb": 1,
                    "target_free_gb": 2,
                    "max_bytes_per_pass": 53687091200_u64,
                    "max_items_per_pass": 50,
                    "max_scan_items": 10000,
                    "cleaners": {}
                },
                "slots": 1
            }));
    }
    fs::write(
        storage.join("registry.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
}
