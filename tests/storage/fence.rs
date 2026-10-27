//! `stado storage fence`: arm, refuse, lift, against one isolated
//! deployment's own local store.
//!
//! `config init` seeds a local store under the deployment's home, and the
//! shared directory template is imported into it. Expected: the store reads
//! as not fenced; `arm` for one transaction records the intent beside the
//! store; a real write (`queue pause`, which writes the pause marker into the
//! store) is refused naming the fence and how to lift it, and the store holds
//! no pause marker; `arm` and `disarm` for another transaction are each
//! refused naming the one that holds it; a registry push is refused without
//! a fence transaction, with another one, and with no `--if-generation`, and
//! lands with the arming one (`--fence-transaction`, the role move's commit
//! through its own fence);
//! `disarm` for the arming transaction removes the intent, and the same
//! write then succeeds and the store holds its marker.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const TRANSACTION: &str = "fence-journey";
const OTHER: &str = "fence-journey-other";

struct Deployment {
    root: PathBuf,
    report: Value,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crate sits inside the repository")
            .to_path_buf();
        let root = repository
            .join(".build/storage-fence")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        Self {
            root,
            report: json!({"commands": [], "outcome": "failed"}),
        }
    }

    fn intent(&self) -> PathBuf {
        self.root
            .join(".stado")
            .join("recovery")
            .join("storage-root-write-fence.json")
    }

    fn run(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn cli(&mut self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn refused(&mut self, args: &[&str], said: &str) {
        let output = self.run(args);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(!output.status.success(), "stado {args:?} must be refused: {stderr}");
        assert!(stderr.contains(said), "stado {args:?} says {said:?}: {stderr}");
    }

    fn queue(&mut self) -> Value {
        serde_json::from_str(&self.cli(&["queue", "status", "--json"])).unwrap()
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn a_fenced_store_refuses_writes_until_its_own_transaction_lifts_the_fence() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let template = deployment.root.join("registry.json");
    fs::write(&template, include_str!("../service_directory/registry.json")).unwrap();
    deployment.cli(&["registry", "import", &template.to_string_lossy()]);
    let unfenced: Value =
        serde_json::from_str(&deployment.cli(&["storage", "fence", "status", "--json"])).unwrap();
    assert!(unfenced["intent"].is_null(), "{unfenced}");

    let armed: Value = serde_json::from_str(&deployment.cli(&[
        "storage", "fence", "arm", "--transaction", TRANSACTION, "--reason", "journey", "--json",
    ]))
    .unwrap();
    assert_eq!(armed["state"], "armed", "{armed}");
    let intent: Value = serde_json::from_slice(&fs::read(deployment.intent()).unwrap()).unwrap();
    assert_eq!(intent["transaction"], TRANSACTION, "{intent}");

    deployment.refused(
        &["queue", "pause", "--reason", "fenced journey"],
        &format!("local storage writes are fenced by stado storage fence transaction {TRANSACTION}"),
    );
    let before = deployment.queue();
    assert_ne!(before["paused"], true, "a fenced store took the pause marker: {before}");
    deployment.refused(
        &["storage", "fence", "arm", "--transaction", OTHER, "--reason", "journey"],
        &format!("already fenced by transaction {TRANSACTION}"),
    );
    deployment.refused(
        &["storage", "fence", "disarm", "--transaction", OTHER, "--reason", "journey"],
        &format!("is fenced by transaction {TRANSACTION}, not {OTHER}"),
    );

    let token = deployment.cli(&["registry", "pull", "--generation-only"]).trim().to_string();
    let mut document: Value = serde_json::from_str(&deployment.cli(&["registry", "pull"])).unwrap();
    document["service_directory"]["authority"]["command"] = json!("/fence-journey/stado");
    let edited = deployment.root.join("registry-edited.json");
    fs::write(&edited, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let edited = edited.to_string_lossy().into_owned();
    let fenced = format!("local storage writes are fenced by stado storage fence transaction {TRANSACTION}");
    deployment.refused(&["registry", "push", &edited, "--if-generation", &token], &fenced);
    deployment.refused(&["registry", "push", &edited, "--if-generation", &token, "--fence-transaction", OTHER], &fenced);
    // The fenced commit is conditional on the generation it was made against.
    deployment.refused(&["registry", "push", &edited, "--fence-transaction", TRANSACTION], "--if-generation");
    deployment.cli(&["registry", "push", &edited, "--if-generation", &token, "--fence-transaction", TRANSACTION]);
    let committed: Value = serde_json::from_str(&deployment.cli(&["registry", "pull"])).unwrap();
    assert_eq!(committed["service_directory"]["authority"]["command"], "/fence-journey/stado", "{committed}");
    deployment.refused(&["queue", "pause", "--reason", "fenced journey"], &fenced);

    let lifted: Value = serde_json::from_str(&deployment.cli(&[
        "storage", "fence", "disarm", "--transaction", TRANSACTION, "--reason", "journey", "--json",
    ]))
    .unwrap();
    assert_eq!(lifted["state"], "disarmed", "{lifted}");
    assert!(!deployment.intent().exists(), "the intent stays after disarm");
    deployment.cli(&["queue", "pause", "--reason", "after the fence"]);
    let after = deployment.queue();
    assert_eq!(after["paused"], true, "the write after disarm did not land: {after}");

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("storage-fence evidence: {}", deployment.root.display());
}
