//! What `stado bootstrap --dry-run` says it would write for a remote host,
//! read through the real binary against an isolated registry: the Linux
//! systemd unit carries the dedicated workload-agent grant declaration, with
//! the bearer at an absolute path under the remote account's home, and the
//! Darwin installer command carries the same declaration.

use std::path::Path;
use std::process::{Command, Output};

const LINUX_TARGET: &str = "linux-fixture";
const DARWIN_TARGET: &str = "darwin-fixture";
/// The control plane's authenticated Skarbiec, as a fleet declares it.
const AGENT_URL: &str = "https://control-plane.fixture.invalid";
const CONSUMER: &str = "stado-local-agent";
/// The registry document schema the live registry carries.
const REGISTRY_SCHEMA_VERSION: u16 = 2;
/// The configuration file schema the product reads.
const CONFIG_SCHEMA_VERSION: u16 = 1;

fn stado(storage: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("HOME", home)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", home.join(".config/stado/config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .env_remove("WC_AGENT_SKARBIEC_URL")
        .env_remove("WC_AGENT_SKARBIEC_CONSUMER")
        .env_remove("WC_AGENT_SKARBIEC_ITEMS")
        .env_remove("WC_AGENT_SKARBIEC_SECRET_FIELDS")
        .output()
        .expect("stado binary runs")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// An isolated registry with one Linux and one Darwin remote target, and a
/// configuration declaring the control plane's dedicated agent grant.
fn fixture() -> (tempfile::TempDir, tempfile::TempDir) {
    let storage = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let document = serde_json::json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "targets": [
            {
                "name": LINUX_TARGET,
                "kind": "local",
                "ssh": "root@10.0.0.108",
                "release_platform": "linux-amd64",
                "hostnames": ["linux-fixture.invalid"]
            },
            {
                "name": DARWIN_TARGET,
                "kind": "local",
                "ssh": "op@10.0.0.234",
                "release_platform": "darwin-arm64",
                "hostnames": ["darwin-fixture.invalid"]
            }
        ],
        "coordinators": []
    });
    std::fs::write(
        storage.path().join("registry.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    let config_dir = home.path().join(".config/stado");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "agent": {
            "skarbiec": {
                "url": AGENT_URL,
                "consumer": CONSUMER,
                "token_file": home.path().join("agent-grant").display().to_string(),
                "items": ["GITHUB_TOKEN", "stado-huggingface"],
                "secret_fields": ["GITHUB_TOKEN#value", "stado-huggingface#token"]
            }
        }
    });
    std::fs::write(config_dir.join("config.json"), config.to_string()).unwrap();
    (storage, home)
}

#[test]
fn the_linux_unit_carries_the_agent_grant_declaration() {
    let (storage, home) = fixture();
    let out = stado(
        storage.path(),
        home.path(),
        &["bootstrap", "--target", LINUX_TARGET, "--dry-run"],
    );
    let text = text(&out);
    assert!(out.status.success(), "{text}");
    for line in [
        format!("Environment=\"WC_AGENT_SKARBIEC_URL={AGENT_URL}\""),
        format!("Environment=\"WC_AGENT_SKARBIEC_CONSUMER={CONSUMER}\""),
        "Environment=\"WC_AGENT_SKARBIEC_TOKEN_FILE=/root/.stado/local-agent-skarbiec-token\""
            .to_string(),
        "Environment=\"WC_AGENT_SKARBIEC_ITEMS=GITHUB_TOKEN,stado-huggingface\"".to_string(),
        "Environment=\"WC_AGENT_SKARBIEC_SECRET_FIELDS=GITHUB_TOKEN#value,stado-huggingface#token\""
            .to_string(),
        format!("Environment=\"WC_SKARBIEC_CONSUMER={CONSUMER}\""),
        "Environment=\"WC_SKARBIEC_TOKEN_FILE=/root/.stado/local-agent-skarbiec-token\""
            .to_string(),
    ] {
        assert!(text.contains(&line), "missing {line:?} in:\n{text}");
    }
    let unit_start = text.find("[Service]").expect("a unit body");
    let exec = text[unit_start..]
        .find("ExecStart=")
        .expect("an ExecStart line");
    let grant = text[unit_start..]
        .find("WC_AGENT_SKARBIEC_URL")
        .expect("the grant line");
    assert!(
        grant < exec,
        "the grant must be declared before the agent starts:\n{text}"
    );
}

#[test]
fn the_darwin_installer_carries_the_same_declaration() {
    let (storage, home) = fixture();
    let out = stado(
        storage.path(),
        home.path(),
        &["bootstrap", "--target", DARWIN_TARGET, "--dry-run"],
    );
    let text = text(&out);
    assert!(out.status.success(), "{text}");
    assert!(
        text.contains(&format!("WC_AGENT_SKARBIEC_CONSUMER={CONSUMER} ")),
        "{text}"
    );
    assert!(
        text.contains("WC_AGENT_SKARBIEC_TOKEN_FILE=/home/op/.stado/local-agent-skarbiec-token"),
        "{text}"
    );
    assert!(text.contains("stado bootstrap --local"), "{text}");
}
