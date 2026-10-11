//! `stado credentials vault show` on a machine whose registry name cannot be
//! read, while the directory names a host that owns the fleet vault, answers
//! `host_unreadable` with a refusal naming the cause and the owner and exits
//! non-zero; it does not skip the owner check and judge this machine's local
//! vault files as if it owned the fleet vault.
//!
//! The real stado runs with an isolated `HOME` under the checkout's build
//! directory and a local store holding the shared directory template, in
//! which no target carries this machine's hostname and the `skarbiec`
//! service is active on the template's last target.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const OWNER: &str = "other-host";
const SERVICE: &str = "skarbiec";
const STATE: &str = "host_unreadable";

struct Deployment {
    root: PathBuf,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/vault-authority-host-unreadable")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        Self { root }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH names the tools"),
            )
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        let name = args.join("-").replace('/', "_");
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }

    fn cli(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// The shared directory template with no target naming this machine and
    /// the fleet vault's service active on the last target, at the endpoint
    /// the template gives that target's own service.
    fn import_registry(&self) {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let targets = document["targets"].as_array_mut().unwrap();
        for target in targets.iter_mut() {
            target["hostnames"] = json!([]);
        }
        let owner = targets.last_mut().unwrap();
        owner["name"] = json!(OWNER);
        let services = owner["services"].as_array_mut().unwrap();
        let owners_service = services
            .first()
            .and_then(|service| service["name"].as_str())
            .unwrap()
            .to_string();
        services.push(json!({"name": SERVICE, "unit": format!("com.wisent.{SERVICE}")}));
        let endpoint =
            document["service_directory"]["services"][&owners_service]["endpoints"][OWNER].clone();
        document["service_directory"]["services"][SERVICE] = json!({
            "managed_service": SERVICE,
            "active_host": OWNER,
            "endpoints": {OWNER: endpoint},
            "consumers": {},
        });
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

#[test]
fn an_unreadable_host_name_is_reported_with_its_cause_and_the_owner_not_skipped() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();

    let output = deployment.run(&["credentials", "vault", "show", "--json"]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "a vault verdict that could not judge the owner exits non-zero: {stdout}{stderr}"
    );
    let report: Value = match serde_json::from_str(&stdout) {
        Ok(report) => report,
        Err(error) => panic!("one JSON report per invocation ({error}): {stdout}{stderr}"),
    };
    assert_eq!(
        report["state"],
        json!(STATE),
        "the owner check that did not run is the state, not a verdict on local files: {report}"
    );
    assert_eq!(
        report["owner_host"],
        json!(OWNER),
        "the fleet vault owner the directory names is reported: {report}"
    );
    let Some(refusal) = report["refusal"].as_str() else {
        panic!("the refusal is a sentence: {report}");
    };
    assert!(
        refusal.contains(OWNER) && refusal.contains("could not be read"),
        "the refusal names the owner and says the host name could not be read: {refusal}"
    );
    assert!(
        refusal.contains(&hostname()),
        "the cause carries the registry's own sentence about this hostname: {refusal}"
    );
    assert_eq!(
        report["path"],
        Value::Null,
        "no vault path is answered for a machine that may not hold the vault: {report}"
    );
}
