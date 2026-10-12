//! The Resend channel on a machine without Skarbiec, through the real
//! `stado` binary against one isolated deployment: a local store holding a
//! registry, a config whose `credentials.store` selects the owner-only file
//! credential store, and no Skarbiec anywhere.
//!
//! With the operator's choice recorded and the destination in config, a key
//! the store does not hold is said by name with the `put` that writes it;
//! `stado credentials put` writes it into the file store, owner-only; the
//! channel then resolves to the destination and `alerts channels --json`
//! reports it, Skarbiec never asked. A store file readable by others is
//! refused as such, never read.
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Deployment {
    root: PathBuf,
}

const RESEND_ROLE: &str = "RESEND_API_KEY";
const DESTINATION: &str = "operator@alerts.example";

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/alerts-file-store")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join(".stado")).unwrap();
        fs::create_dir_all(root.join("store/ecosystem/probierz")).unwrap();
        let deployment = Self { root };
        let registry: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        fs::write(
            deployment
                .root
                .join("store/ecosystem/probierz/registry.json"),
            serde_json::to_vec_pretty(&registry).unwrap(),
        )
        .unwrap();
        fs::write(
            deployment.root.join(".stado/config.json"),
            serde_json::to_vec_pretty(&json!({
                "credentials": { "store": format!("file://{}", deployment.store_path().display()) },
                "alerts": { "email_to": DESTINATION },
            }))
            .unwrap(),
        )
        .unwrap();
        deployment
    }

    fn store_path(&self) -> PathBuf {
        self.root.join(".stado/credentials.json")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"));
        command
    }

    fn stado(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    fn stado_with_stdin(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

fn said(output: &Output) -> String {
    format!(
        "exit {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn says(output: &Output, sentence: &str) {
    let text = said(output);
    assert!(text.contains(sentence), "expected {sentence:?} in:\n{text}");
}

#[test]
fn the_resend_channel_resolves_from_the_file_store_without_skarbiec() {
    let deployment = Deployment::start();
    let chosen = deployment.stado(&["alerts", "preferences", "set", "--channel", "resend"]);
    assert!(chosen.status.success(), "{}", said(&chosen));

    let unkeyed = deployment.stado(&["alerts", "channels"]);
    says(&unkeyed, "no alert channel resolves");
    says(
        &unkeyed,
        &format!("the selected credential store gave no value for role {RESEND_ROLE}"),
    );
    says(&unkeyed, &format!("stado credentials put {RESEND_ROLE}"));
    let text = said(&unkeyed);
    assert!(
        !text.contains("Skarbiec request failed"),
        "Skarbiec was asked on a machine without it:\n{text}"
    );

    let written =
        deployment.stado_with_stdin(&["credentials", "put", RESEND_ROLE], "re_test_key\n");
    assert!(written.status.success(), "{}", said(&written));

    let resolved = deployment.stado(&["alerts", "channels", "--json"]);
    assert!(resolved.status.success(), "{}", said(&resolved));
    let channels: Value = serde_json::from_slice(&resolved.stdout).unwrap();
    let destination = channels["resend"].as_str().unwrap();
    assert!(destination.starts_with(DESTINATION), "{channels}");
    let text = said(&resolved);
    assert!(
        !text.contains("resend-configuration"),
        "the channel resolved with a failure line:\n{text}"
    );

    let stored: Value =
        serde_json::from_slice(&fs::read(deployment.store_path()).unwrap()).unwrap();
    assert_eq!(stored[RESEND_ROLE]["value"], "re_test_key");

    // The store opened to the group and others, as a directory the deployment
    // created is: the read is refused by the file's own check, never served.
    let mut open = fs::metadata(deployment.store_path()).unwrap().permissions();
    open.set_mode(
        open.mode()
            | fs::metadata(deployment.root.join("tmp"))
                .unwrap()
                .permissions()
                .mode(),
    );
    fs::set_permissions(deployment.store_path(), open).unwrap();
    let refused = deployment.stado(&["alerts", "channels"]);
    says(&refused, "no alert channel resolves");
    says(&refused, "resend-configuration");
    says(&refused, "credentials.json");

    fs::remove_dir_all(&deployment.root).unwrap();
}
