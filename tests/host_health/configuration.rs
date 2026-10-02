//! Read-only journey against two real registry hosts. One serves its effective
//! configuration; the other has a real configuration-command refusal. No host
//! files, services, grants or credentials are changed by this journey.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Journey {
    root: PathBuf,
    report: Value,
}

impl Journey {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/host-configuration")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let mut journey = Self {
            root,
            report: json!({"outcome": "failed", "commands": [], "inputs": {}}),
        };
        let revision = Command::new("git")
            .current_dir(&repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(revision.status.success());
        let patch = Command::new("git")
            .current_dir(&repository)
            .args(["diff", "--binary", "HEAD"])
            .output()
            .unwrap();
        assert!(patch.status.success());
        fs::write(journey.root.join("source.patch"), &patch.stdout).unwrap();
        journey.report["source_revision"] =
            json!(String::from_utf8(revision.stdout).unwrap().trim());
        journey.report["source_patch_sha256"] =
            json!(format!("{:x}", Sha256::digest(&patch.stdout)));
        let mut binary = File::open(env!("CARGO_BIN_EXE_stado")).unwrap();
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            let count = binary.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        journey.report["binary_sha256"] = json!(format!("{:x}", digest.finalize()));
        let version = journey.run(&["--version"]);
        assert!(version.status.success());
        let version = String::from_utf8(version.stdout).unwrap();
        assert!(
            version
                .split_whitespace()
                .map(|part| part.trim_matches(['(', ')']))
                .map(|part| part.strip_suffix("-dirty").unwrap_or(part))
                .any(|part| Some(part) == journey.report["source_revision"].as_str()),
            "the tested executable must identify this source revision: {version}"
        );
        journey.report["binary_version"] = json!(version);
        journey.save();
        journey
    }

    fn input(&mut self, name: &str) -> String {
        match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => {
                self.report["inputs"][name] = json!(value);
                self.save();
                value
            }
            _ => {
                self.report["outcome"] = json!("blocked");
                self.report["missing_prerequisite"] = json!(name);
                self.save();
                panic!("real host journey requires {name}");
            }
        }
    }

    fn run(&mut self, arguments: &[&str]) -> Output {
        let result = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .stdin(Stdio::null())
            .output();
        match result {
            Ok(output) => {
                self.report["commands"].as_array_mut().unwrap().push(json!({
                    "arguments": arguments, "exit_status": output.status.code(),
                    "stdout": String::from_utf8_lossy(&output.stdout),
                    "stderr": String::from_utf8_lossy(&output.stderr)
                }));
                self.save();
                output
            }
            Err(error) => {
                self.report["commands"].as_array_mut().unwrap().push(json!({
                    "arguments": arguments, "spawn_error": error.to_string()
                }));
                self.save();
                panic!("real Stado command could not start: {error}");
            }
        }
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Journey {
    fn drop(&mut self) {
        self.save();
        eprintln!("host configuration evidence: {}", self.root.display());
    }
}

#[test]
#[ignore = "requires real readable and configuration-refusing registry hosts; run explicitly with --ignored"]
fn effective_configuration_and_dependent_credential_refusal() {
    let mut journey = Journey::start();
    let readable = journey.input("STADO_HOST_CONFIG_TARGET");
    let expected_file = journey.input("STADO_HOST_CONFIG_EXPECTED_FILE");
    let expected_vault = journey.input("STADO_HOST_CONFIG_EXPECTED_VAULT");
    let refused = journey.input("STADO_HOST_CONFIG_REFUSAL_TARGET");
    let item = journey.input("STADO_HOST_CONFIG_ITEM");
    let remote_exit = journey
        .input("STADO_HOST_CONFIG_REMOTE_EXIT")
        .parse::<i32>()
        .expect("remote exit status must be an integer");
    assert_ne!(
        remote_exit, 0,
        "the second host must have a real remote refusal"
    );
    assert_ne!(
        readable, refused,
        "use distinct hosts for the two observed states"
    );
    let shown = journey.run(&["host", "config-show", &readable, "--json"]);
    assert!(
        shown.status.success(),
        "the readable host did not return its configuration"
    );
    let configuration: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(configuration["file"], expected_file);
    assert_eq!(
        configuration["resolved"]["skarbiec_vault_file"],
        expected_vault
    );
    journey.report["observed_configuration"] = configuration;
    journey.save();

    let inspection = journey.run(&[
        "credentials",
        "item",
        "show",
        "--host",
        &refused,
        &item,
        "--field",
        "token",
        "--json",
    ]);
    assert!(
        !inspection.status.success(),
        "an unreadable host declaration must not yield credential metadata"
    );
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&inspection.stdout),
        String::from_utf8_lossy(&inspection.stderr)
    );
    // These are provenance values, not a pinned human sentence: a nested
    // refusal must identify the selected host, actual remote command and
    // child status instead of blaming the outer credentials arguments.
    assert!(
        diagnostic.contains(&refused),
        "the failed host is absent from the diagnostic"
    );
    assert!(
        diagnostic.contains("config show --json"),
        "the failed configuration read is absent from the diagnostic"
    );
    assert!(
        diagnostic.contains("~/.stado/bin/stado"),
        "the remote executable is absent from the diagnostic"
    );
    assert!(
        diagnostic.contains(&format!("exit {remote_exit}")),
        "the real remote exit status is absent from the diagnostic"
    );
    journey.report["outcome"] = json!("passed");
    journey.save();
}
