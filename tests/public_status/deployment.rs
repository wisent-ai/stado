//! One isolated deployment for the public-status journeys: a local object
//! store and a home of their own, the built stado run inside them, and every
//! command with its exit status and output kept in the deployment's
//! `report.json` under the checkout's `.build`.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

pub struct Deployment {
    pub root: PathBuf,
    pub report: Value,
}

impl Deployment {
    pub fn start(name: &str) -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build")
            .join(format!("public-status-{name}"))
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store")).unwrap();
        let mut deployment = Self {
            root,
            report: json!({
                "source_revision": std::env::var("STADO_SOURCE_REVISION")
                    .expect("declare the committed candidate"),
                "commands": [],
                "outcome": "failed",
            }),
        };
        let init = deployment.run(&["config", "init"]);
        assert!(init.status.success(), "{init:?}");
        deployment
    }

    /// The built stado inside this deployment, not yet started.
    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null());
        command
    }

    pub fn run(&mut self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    pub fn json(&mut self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(output.status.success(), "stado {args:?}: {output:?}");
        serde_json::from_slice(&output.stdout).expect("the answer is JSON")
    }

    pub fn passed(&mut self) {
        self.report["outcome"] = json!("passed");
        self.save();
    }

    pub fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

/// Every `(product, component, title)` the compiled catalog declares.
pub fn declared() -> Vec<(String, String, String)> {
    let catalog = stado_product::catalog::embedded().expect("the compiled catalog is valid");
    let mut declared = Vec::new();
    for product in catalog["products"].as_array().unwrap() {
        for check in product["public_status"].as_array().into_iter().flatten() {
            declared.push((
                product["id"].as_str().unwrap().to_owned(),
                check["component"].as_str().unwrap().to_owned(),
                check["title"].as_str().unwrap().to_owned(),
            ));
        }
    }
    assert!(!declared.is_empty(), "the catalog declares components");
    declared
}

/// The published row of `product`/`component`.
pub fn row<'a>(document: &'a Value, product: &str, component: &str) -> &'a Value {
    let Some(row) = document["components"].as_array().and_then(|rows| {
        rows.iter()
            .find(|row| row["product"] == product && row["component"] == component)
    }) else {
        panic!("{product}/{component} is published: {document}");
    };
    row
}
