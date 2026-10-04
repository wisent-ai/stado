//! `service_api.deployers` refuses one unit claimed by two products and
//! accepts one product that still lists several labels it ran under before.
//! Each case is an isolated deployment: `config init` seeds a fresh config,
//! the real `stado` executable writes the deployers with `config set`, and
//! `config validate` judges the file it wrote. A vault host whose
//! `compute-marketplace` deployer kept its three retired agent labels made
//! stado 0.23.19 refuse that host's configuration on install, which left the
//! old stado running.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Deployment {
    root: PathBuf,
    report: Value,
}

impl Deployment {
    fn start(case: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".build/config-deployers")
            .join(format!("{case}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("tmp")).unwrap();
        Self {
            root,
            report: json!({"case": case, "commands": [], "outcome": "failed"}),
        }
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

    /// `config set` then `config validate`; the combined stderr and whether
    /// both commands succeeded.
    fn declare(&mut self, deployers: &Value) -> (bool, String) {
        let init = self.run(&["config", "init"]);
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        let set = self.run(&[
            "config",
            "set",
            "service_api.deployers",
            &deployers.to_string(),
        ]);
        let mut stderr = String::from_utf8_lossy(&set.stderr).into_owned();
        if !set.status.success() {
            return (false, stderr);
        }
        let validate = self.run(&["config", "validate"]);
        stderr.push_str(&String::from_utf8_lossy(&validate.stderr));
        stderr.push_str(&String::from_utf8_lossy(&validate.stdout));
        (validate.status.success(), stderr)
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }

    fn passed(mut self) {
        self.report["outcome"] = json!("passed");
        self.save();
        eprintln!("config-deployers evidence: {}", self.root.display());
    }
}

#[test]
fn one_product_listing_its_retired_labels_is_accepted() {
    let mut deployment = Deployment::start("retired-labels");
    let (accepted, stderr) = deployment.declare(&json!({
        "compute-marketplace": {
            "item": "compute-marketplace",
            "services": [
                "com.wisent.compute-marketplace",
                "compute-marketplace-agent-a",
                "compute-marketplace-agent-b"
            ],
            "actions": ["status", "restart"]
        }
    }));
    assert!(
        accepted,
        "labels one product ran under are its one unit: {stderr}"
    );
    assert!(!stderr.contains("more than one deployer"), "{stderr}");
    deployment.passed();
}

#[test]
fn one_unit_claimed_by_two_products_is_refused_naming_both() {
    let mut deployment = Deployment::start("two-products");
    let (accepted, stderr) = deployment.declare(&json!({
        "compute-marketplace": {
            "item": "compute-marketplace",
            "services": ["com.wisent.compute-marketplace"],
            "actions": ["status"]
        },
        "stado": {
            "item": "stado",
            "services": ["com.wisent.compute-marketplace"],
            "actions": ["status"]
        }
    }));
    assert!(!accepted, "two products may not deploy one unit: {stderr}");
    assert!(
        stderr.contains(
            "service \"com.wisent.compute-marketplace\" is mapped to more than one deployer: \
             compute-marketplace and stado"
        ),
        "the refusal names the unit and both deployers: {stderr}"
    );
    deployment.passed();
}
